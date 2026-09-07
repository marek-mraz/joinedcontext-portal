import { lazy, Suspense, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { Feature, FeatureCollection } from "geojson";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import type { Manifest } from "../api/manifest";
import { rendersWithDeckGl } from "../components/dashboards/rendering";
import type { MapLayer } from "../components/dashboards/MapLibreView";
import type { DenseLayer } from "../components/dashboards/DeckGlOverlay";

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

/** The Endpoint's GeoJSON projection is where a layer's features come from (UI-22). */
function geojsonUrl(slug: string, spec: LayerSpec): string {
  const params = new URLSearchParams();
  if (spec.entityType) {
    params.set("type", spec.entityType);
  }
  for (const key of ["q", "scopeQ", "geoQ"] as const) {
    const value = spec.filter?.[key];
    if (value) {
      params.set(key, value);
    }
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

export function DashboardsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [selected, setSelected] = useState<string | null>(null);

  const dashboards = useResourceList(project, "dashboards");
  const layers = useResourceList(project, "layers");
  const endpoints = useResourceList(project, "endpoints");

  const dashboard = useMemo(() => {
    const items = asManifests(dashboards.data?.items ?? []).filter(
      (item) => item.kind === "Dashboard",
    );
    return items.find((item) => item.metadata.name === selected) ?? items[0];
  }, [dashboards.data, selected]);

  const { mapLayers, blocked } = useMemo(() => {
    const spec = (dashboard?.spec ?? {}) as DashboardSpec;
    const page = spec.pages?.[0];
    const layerByName = new Map(
      asManifests(layers.data?.items ?? [])
        .filter((item) => item.kind === "Layer")
        .map((item) => [item.metadata.name, item] as const),
    );
    const endpointByName = new Map(
      asManifests(endpoints.data?.items ?? []).map((item) => [item.metadata.name, item] as const),
    );

    const resolved: PlannedLayer[] = [];
    const refused: string[] = [];
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
      // A public dashboard may only read from a public Endpoint (UI-19). The forge CI
      // rejects the manifest too; the view refuses to ask for the data either way.
      if (spec.visibility === "public" && endpointSpec.audience !== "public") {
        refused.push(name);
        continue;
      }
      if (layerSpec.visible === false) {
        continue;
      }
      resolved.push({
        name,
        url: geojsonUrl(endpointSpec.slug, layerSpec),
        style: STYLES.find((style) => style === layerSpec.style) ?? "circle",
        colorBy: layerSpec.colorBy,
        sizeBy: layerSpec.sizeBy,
        popupProperties: layerSpec.popupProperties,
      });
    }
    return { mapLayers: resolved, blocked: refused };
  }, [dashboard, layers.data, endpoints.data]);

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
    return <p role="status">{t("app.loading")}</p>;
  }

  if (dashboards.isError) {
    const message =
      dashboards.error instanceof ApiError
        ? (dashboards.error.problem?.detail ?? dashboards.error.message)
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <button
          type="button"
          onClick={() => {
            void dashboards.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  const all = asManifests(dashboards.data.items ?? []).filter((item) => item.kind === "Dashboard");

  if (all.length === 0 || !dashboard) {
    return (
      <div className="space-y-4">
        <h1 className="text-xl font-bold">{t("dashboards.title")}</h1>
        <p className="text-sm text-surface-fg/70">{t("dashboards.empty")}</p>
      </div>
    );
  }

  const spec = dashboard.spec as DashboardSpec;
  const title = localized(spec.title, locale, dashboard.metadata.name);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <h1 className="text-xl font-bold">{title}</h1>
        {all.length > 1 ? (
          <label className="flex items-center gap-2 text-sm">
            {t("dashboards.title")}
            <select
              value={dashboard.metadata.name}
              onChange={(event) => setSelected(event.target.value)}
              className="rounded border border-border bg-surface px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {all.map((item: Manifest) => (
                <option key={item.metadata.name} value={item.metadata.name}>
                  {localized((item.spec as DashboardSpec).title, locale, item.metadata.name)}
                </option>
              ))}
            </select>
          </label>
        ) : null}
      </div>

      {blocked.length > 0 ? (
        <p role="status" className="text-sm text-danger">
          {t("dashboards.layerBlocked", { layers: blocked.join(", ") })}
        </p>
      ) : null}

      <Suspense fallback={<p role="status">{t("app.loading")}</p>}>
        {features.isPending && mapLayers.length > 0 ? (
          <p role="status">{t("app.loading")}</p>
        ) : dense.length > 0 ? (
          <DeckGlOverlay layers={native} dense={dense} label={title} />
        ) : (
          <MapLibreView layers={native} label={title} />
        )}
      </Suspense>

      <ul className="flex flex-wrap gap-4">
        {[...native, ...dense].map((layer) => (
          <li key={layer.name} className="rounded border border-border px-3 py-2 text-sm">
            <span className="font-medium">{layer.name}</span>
            {layer.colorBy ? (
              <span className="ml-2 text-surface-fg/70">
                {t("dashboards.colouredBy", {
                  property: layer.colorBy.property,
                  min: layer.colorBy.domain?.[0] ?? 0,
                  max: layer.colorBy.domain?.[1] ?? 100,
                })}
              </span>
            ) : null}
          </li>
        ))}
      </ul>
    </div>
  );
}
