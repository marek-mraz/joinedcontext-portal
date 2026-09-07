import { lazy, Suspense, useCallback, useId, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { Feature, FeatureCollection } from "geojson";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { usePermissions } from "../api/permissions";
import { ChangeNotice } from "../components/ChangeNotice";
import { rendersWithDeckGl } from "../components/dashboards/rendering";
import { RAMP } from "../components/dashboards/MapLibreView";
import type { Bbox, MapLayer } from "../components/dashboards/MapLibreView";
import type { DenseLayer } from "../components/dashboards/DeckGlOverlay";
import {
  DashboardEditor,
  dashboardFromManifest,
  LayerEditor,
  layerFromManifest,
} from "../pages/dashboards/editors";
import type { DashboardForm, LayerForm } from "../pages/dashboards/editors";
import {
  Alert,
  Badge,
  Button,
  Card,
  EmptyState,
  Icon,
  PageHeader,
  Select,
  Skeleton,
} from "../components/ui";

// MapLibre is about half of the bundle; nobody downloads it before opening a dashboard.
const MapLibreView = lazy(() =>
  import("../components/dashboards/MapLibreView").then((module) => ({
    default: module.MapLibreView,
  })),
);

// deck.gl is the other half again, and only a dense dashboard ever needs it.
const DeckGlOverlay = lazy(() =>
  import("../components/dashboards/DeckGlOverlay").then((module) => ({
    default: module.DeckGlOverlay,
  })),
);

/** The styles a Layer manifest may name; `icon` has no renderer yet and draws as a circle. */
const STYLES = ["circle", "line", "fill", "hexagon", "heatmap"] as const;

type LayerStyle = (typeof STYLES)[number];

/** One resolved layer before its features are known, which is what decides the renderer. */
type PlannedLayer = Omit<DenseLayer, "features">;

interface LayerSpec {
  sourceEndpointRef?: string;
  entityType?: string;
  style?: LayerStyle | "icon";
  visible?: boolean;
  filter?: { q?: string; scopeQ?: string; geoQ?: string };
  colorBy?: { property: string; domain?: [number, number]; palette?: string };
  sizeBy?: { property: string; range?: [number, number] };
  popupProperties?: string[];
}

interface DashboardSpec {
  title?: Record<string, string>;
  visibility?: "private" | "project" | "organization" | "public";
  pages?: { title?: string; layout?: string; layers?: string[] }[];
}

/**
 * The Endpoint's GeoJSON projection is where a layer's features come from (UI-22). The
 * spatial constraint is the manifest's own `geoQ` when it declares one, else the viewport
 * once the reader moved the map; the gateway reads it as `georel`, `geometry`, `coordinates`.
 */
export function geojsonUrl(slug: string, spec: LayerSpec, bbox?: Bbox): string {
  const params = new URLSearchParams();
  if (spec.entityType) {
    params.set("type", spec.entityType);
  }
  for (const key of ["q", "scopeQ"] as const) {
    const value = spec.filter?.[key];
    if (value) {
      params.set(key, value);
    }
  }
  if (spec.filter?.geoQ) {
    // `georel` may itself carry a `;` (`near;maxDistance==2000`), so the split is on the
    // three parameter names, not on the separator.
    const parts = spec.filter.geoQ.split(/(?:^|;)(?=(?:georel|geometry|coordinates)=)/);
    for (const part of parts) {
      const at = part.indexOf("=");
      if (at > 0) {
        params.set(part.slice(0, at).trim(), part.slice(at + 1));
      }
    }
  } else if (bbox) {
    const [w, s, e, n] = bbox;
    params.set("georel", "within");
    params.set("geometry", "Polygon");
    params.set("coordinates", JSON.stringify([[[w, s], [e, s], [e, n], [w, n], [w, s]]]));
  }
  const query = params.toString();
  return `/api/endpoint/${slug}/file.geojson${query ? `?${query}` : ""}`;
}

/**
 * Every layer's features, fetched once (UI-21, UI-22).
 *
 * The count is only knowable from the data, and deck.gl needs the data anyway, so the page
 * fetches what MapLibre would otherwise have fetched for itself and hands it on. A body that
 * is not a FeatureCollection yields `null`: the layer then keeps its URL and MapLibre fetches
 * it the old way, which is the right answer for an Endpoint that answered with a problem
 * document.
 */
async function fetchFeatures(url: string): Promise<Feature[] | null> {
  const response = await fetch(url, { credentials: "same-origin" });
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText);
  }
  const body = (await response.json()) as FeatureCollection | null;
  return Array.isArray(body?.features) ? body.features : null;
}

/** A dashboard is three lists deep: the dashboard, its layers, and the endpoints they read. */
function useResourceList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
}

/** The map's frame while its features or its bundle are on the way. */
function MapPlaceholder({ label }: { label: string }): JSX.Element {
  return (
    <div role="status" aria-label={label} className="relative h-[60vh] min-h-[20rem] w-full">
      <Skeleton className="absolute inset-0 rounded-none" />
      <span className="sr-only">{label}</span>
    </div>
  );
}

export function DashboardsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [selected, setSelected] = useState<string | null>(null);
  const selectId = useId();
  // The viewport after the reader moved the map, and the layers switched off in the legend.
  const [bbox, setBbox] = useState<Bbox | undefined>(undefined);
  const [hidden, setHidden] = useState<string[] | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [editingDashboard, setEditingDashboard] = useState<DashboardForm | null>(null);
  const [editingLayer, setEditingLayer] = useState<LayerForm | null>(null);
  const [isNew, setIsNew] = useState(false);
  const permissions = usePermissions(project);
  const mayEditDashboard = permissions.can("Dashboard", "propose");
  const mayEditLayer = permissions.can("Layer", "propose");

  const dashboards = useResourceList(project, "dashboards");
  const layers = useResourceList(project, "layers");
  const endpoints = useResourceList(project, "endpoints");
  const spaces = useResourceList(project, "spaces");
  const models = useResourceList(project, "datamodels");
  const onMoveEnd = useCallback((next: Bbox) => setBbox(next), []);

  const dashboard = useMemo(() => {
    const items = asManifests(dashboards.data?.items ?? []).filter(
      (item) => item.kind === "Dashboard",
    );
    return items.find((item) => item.metadata.name === selected) ?? items[0];
  }, [dashboards.data, selected]);

  const layerManifests = useMemo(
    () => asManifests(layers.data?.items ?? []).filter((item) => item.kind === "Layer"),
    [layers.data],
  );
  const endpointManifests = useMemo(() => asManifests(endpoints.data?.items ?? []), [endpoints.data]);
  const spaceManifests = useMemo(() => asManifests(spaces.data?.items ?? []), [spaces.data]);
  const modelManifests = useMemo(() => asManifests(models.data?.items ?? []), [models.data]);

  const { mapLayers, blocked, legend } = useMemo(() => {
    const spec = (dashboard?.spec ?? {}) as DashboardSpec;
    const page = spec.pages?.[0];
    const layerByName = new Map(layerManifests.map((item) => [item.metadata.name, item] as const));
    const endpointByName = new Map(endpointManifests.map((item) => [item.metadata.name, item] as const));

    const resolved: PlannedLayer[] = [];
    const refused: string[] = [];
    const listed: { name: string; manifest: Manifest; off: boolean; spec: LayerSpec }[] = [];
    for (const name of page?.layers ?? []) {
      const layer = layerByName.get(name);
      const layerSpec = (layer?.spec ?? {}) as LayerSpec;
      const endpoint = layerSpec.sourceEndpointRef
        ? endpointByName.get(layerSpec.sourceEndpointRef)
        : undefined;
      const endpointSpec = (endpoint?.spec ?? {}) as { slug?: string; audience?: string };
      if (!layer || !endpointSpec.slug) {
        refused.push(name);
        continue;
      }
      // A public dashboard may only read from a public Endpoint (UI-19). The Portal refuses
      // the manifest at write time too; the view refuses to ask for the data either way.
      if (spec.visibility === "public" && endpointSpec.audience !== "public") {
        refused.push(name);
        continue;
      }
      // The legend's switch, or the manifest's `visible: false` until the reader touched it.
      const off = hidden ? hidden.includes(name) : layerSpec.visible === false;
      listed.push({ name, manifest: layer, off, spec: layerSpec });
      if (off) {
        continue;
      }
      resolved.push({
        name,
        url: geojsonUrl(endpointSpec.slug, layerSpec, bbox),
        style: STYLES.find((style) => style === layerSpec.style) ?? "circle",
        colorBy: layerSpec.colorBy,
        sizeBy: layerSpec.sizeBy,
        popupProperties: layerSpec.popupProperties,
      });
    }
    return { mapLayers: resolved, blocked: refused, legend: listed };
  }, [dashboard, layerManifests, endpointManifests, hidden, bbox]);

  function toggle(name: string, on: boolean) {
    const current = hidden ?? legend.filter((entry) => entry.off).map((entry) => entry.name);
    setHidden(on ? current.filter((n) => n !== name) : [...current, name]);
  }

  const features = useQuery({
    queryKey: ["layer-features", mapLayers.map((layer) => layer.url)],
    queryFn: () => Promise.all(mapLayers.map((layer) => fetchFeatures(layer.url))),
    enabled: mapLayers.length > 0,
    // A dashboard is opened, read and left. Refetching a 60 000-feature layer whenever the
    // window regains focus is not what the reader came for.
    staleTime: 60_000,
    retry: false,
  });

  const { native, dense } = useMemo(() => {
    const drawn: MapLayer[] = [];
    const overlaid: DenseLayer[] = [];
    mapLayers.forEach((layer, index) => {
      const fetched = features.data?.[index] ?? null;
      if (fetched && rendersWithDeckGl(layer.style, fetched.length)) {
        overlaid.push({ ...layer, features: fetched });
        return;
      }
      // Whatever is left is MapLibre's, and MapLibre knows three styles: an aggregation
      // that never reached the threshold draws as the points it aggregates.
      const style = layer.style === "line" || layer.style === "fill" ? layer.style : "circle";
      // A bare array is not GeoJSON: MapLibre takes a URL, a Feature or a collection.
      const data = fetched ? { type: "FeatureCollection", features: fetched } : undefined;
      drawn.push({ ...layer, style, data });
    });
    return { native: drawn, dense: overlaid };
  }, [mapLayers, features.data]);

  if (dashboards.isPending) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("dashboards.title")} description={t("dashboards.lead")} />
        <Card flush className="overflow-hidden">
          <MapPlaceholder label={t("app.loading")} />
        </Card>
      </div>
    );
  }

  if (dashboards.isError) {
    const message =
      dashboards.error instanceof ApiError
        ? (dashboards.error.problem?.detail ?? dashboards.error.message)
        : t("app.error.generic");
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("dashboards.title")} description={t("dashboards.lead")} />
        <Alert
          role="alert"
          tone="danger"
          actions={
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() => {
                void dashboards.refetch();
              }}
            >
              {t("app.error.retry")}
            </Button>
          }
        >
          {message}
        </Alert>
      </div>
    );
  }

  const all = asManifests(dashboards.data.items ?? []).filter((item) => item.kind === "Dashboard");

  const editors = (
    <>
      <DashboardEditor
        project={project}
        editing={editingDashboard}
        isNew={isNew}
        onEditingChange={setEditingDashboard}
        onChange={setChange}
        layers={layerManifests.map((item) => item.metadata.name)}
      />
      <LayerEditor
        project={project}
        editing={editingLayer}
        isNew={isNew}
        onEditingChange={setEditingLayer}
        onChange={setChange}
        endpoints={endpointManifests}
        spaces={spaceManifests}
        models={modelManifests}
      />
    </>
  );
  const newButtons = (
    <>
      {mayEditDashboard ? (
        <Button
          size="sm"
          variant="primary"
          icon={<Icon name="plus" className="size-4" />}
          onClick={() => {
            setIsNew(true);
            setEditingDashboard(dashboardFromManifest(undefined));
          }}
        >
          {t("dashboards.add")}
        </Button>
      ) : null}
      {mayEditLayer ? (
        <Button
          size="sm"
          icon={<Icon name="plus" className="size-4" />}
          onClick={() => {
            setIsNew(true);
            setEditingLayer(layerFromManifest(undefined));
          }}
        >
          {t("dashboards.addLayer")}
        </Button>
      ) : null}
    </>
  );

  if (all.length === 0 || !dashboard) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader
          title={t("dashboards.title")}
          description={t("dashboards.lead")}
          aside={<div className="flex flex-wrap gap-2">{newButtons}</div>}
        />
        {change ? <ChangeNotice change={change} project={project} /> : null}
        <EmptyState icon="dashboards" title={t("dashboards.empty")} />
        {editors}
      </div>
    );
  }

  const spec = dashboard.spec as DashboardSpec;
  const title = localized(spec.title, locale, dashboard.metadata.name);

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={title}
        description={t("dashboards.lead")}
        aside={
          <div className="flex flex-wrap items-center gap-2">
            {all.length > 1 ? (
              <label htmlFor={selectId} className="flex items-center gap-2 text-body text-fg-muted">
                <span className="whitespace-nowrap">{t("dashboards.title")}</span>
                <Select
                  id={selectId}
                  value={dashboard.metadata.name}
                  onChange={(event) => {
                    setSelected(event.target.value);
                    setHidden(null);
                  }}
                  className="min-w-[14rem]"
                >
                  {all.map((item: Manifest) => (
                    <option key={item.metadata.name} value={item.metadata.name}>
                      {localized((item.spec as DashboardSpec).title, locale, item.metadata.name)}
                    </option>
                  ))}
                </Select>
              </label>
            ) : null}
            {mayEditDashboard ? (
              <Button
                size="sm"
                onClick={() => {
                  setIsNew(false);
                  setEditingDashboard(dashboardFromManifest(dashboard));
                }}
              >
                {t("dashboards.edit")}
              </Button>
            ) : null}
            {newButtons}
          </div>
        }
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      {blocked.length > 0 ? (
        <Alert role="status" tone="warning">
          {t("dashboards.layerBlocked", { layers: blocked.join(", ") })}
        </Alert>
      ) : null}

      <Card flush className="overflow-hidden">
        <Suspense fallback={<MapPlaceholder label={t("app.loading")} />}>
          {features.isPending && mapLayers.length > 0 ? (
            <MapPlaceholder label={t("app.loading")} />
          ) : dense.length > 0 ? (
            <DeckGlOverlay layers={native} dense={dense} label={title} />
          ) : (
            <MapLibreView layers={native} label={title} onMoveEnd={onMoveEnd} />
          )}
        </Suspense>
      </Card>

      {legend.length > 0 ? (
        <ul aria-label={t("dashboards.legend")} className="flex flex-wrap gap-2">
          {legend.map((entry) => (
            <li
              key={entry.name}
              className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface px-3 py-2 text-body shadow-1"
            >
              <label className="inline-flex items-center gap-2">
                <input
                  type="checkbox"
                  aria-label={`${t("dashboards.show")}: ${entry.name}`}
                  checked={!entry.off}
                  onChange={(event) => toggle(entry.name, event.target.checked)}
                />
                <Badge tone="primary" mono>
                  {entry.spec.style ?? "circle"}
                </Badge>
                <span className="font-medium">{entry.name}</span>
              </label>
              {entry.spec.colorBy ? (
                <span className="inline-flex items-center gap-1 text-caption text-fg-muted">
                  <span>{entry.spec.colorBy.domain?.[0] ?? 0}</span>
                  {RAMP.map((stop) => (
                    <span
                      key={stop}
                      aria-hidden="true"
                      className="inline-block size-3 rounded-sm border border-border"
                      style={{ backgroundColor: stop }}
                    />
                  ))}
                  <span>{entry.spec.colorBy.domain?.[1] ?? 100}</span>
                  <span>
                    {t("dashboards.colouredBy", {
                      property: entry.spec.colorBy.property,
                      min: entry.spec.colorBy.domain?.[0] ?? 0,
                      max: entry.spec.colorBy.domain?.[1] ?? 100,
                    })}
                  </span>
                </span>
              ) : null}
              {mayEditLayer ? (
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`${t("dashboards.editLayer")}: ${entry.name}`}
                  onClick={() => {
                    setIsNew(false);
                    setEditingLayer(layerFromManifest(entry.manifest));
                  }}
                >
                  {t("dashboards.edit")}
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      {editors}
    </div>
  );
}
