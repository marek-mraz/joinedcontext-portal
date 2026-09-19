/**
 * The SDK's entity grid inside the Portal (T-1439; UI-71, SDK-29): one component, linked and not
 * copied, with everything a host owes it supplied here.
 *
 * The grid itself knows nothing about the Portal: it takes its strings, its source and its state
 * from whoever renders it. This adapter is that whoever — the Portal's i18n for every label, the
 * Portal's own origin transport (so the person's session and CSRF header travel as with every other
 * Portal call, and the Endpoint's Policy decides each read), and the column layout a person set
 * kept per project and per type. Every Portal place that shows entities renders this, so a change
 * to the grid reaches all of them and none of them forks it.
 */
import { useCallback, useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { EntityGrid, DEFAULT_LABELS, originTransport, sourceFor } from "@joinedcontext/sdk";
import type { EntityGridProps, GeoLabels, GridLabels, GridMapLabels, GridSource, GridState, ResolvedGridConfig } from "@joinedcontext/sdk";

/** Where the layout a person set is kept, per project and per type. */
function layoutKey(project: string, type: string): string {
  return `jc.grid.${project}.${type}`;
}

/**
 * The parts of the grid's state that belong to the person rather than to the query: which metadata
 * columns are shown and how the page is ordered. The offset, the filters and the pending edits are
 * deliberately not kept: coming back to a page three of a filtered answer that no longer matches is
 * worse than starting at the top, and an unapplied edit must not outlive the page it was typed on.
 */
type Layout = Pick<GridState, "shown" | "sort">;

function readLayout(project: string, type: string): Layout | null {
  try {
    const raw = window.localStorage.getItem(layoutKey(project, type));
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw) as Layout;
    return typeof parsed === "object" && parsed !== null ? parsed : null;
  } catch {
    // A private window, blocked site data, or a value another version wrote: the grid opens with
    // its own defaults rather than failing to open.
    return null;
  }
}

function writeLayout(project: string, type: string, layout: Layout): void {
  try {
    window.localStorage.setItem(layoutKey(project, type), JSON.stringify(layout));
  } catch {
    // Nothing else changes: a layout that cannot be kept is a convenience, not the data.
  }
}

/** Every string the grid shows, from the Portal's own catalogue (`entityGrid.*`). */
export function gridLabels(t: (key: string) => string): GridLabels {
  const ops = DEFAULT_LABELS.ops;
  return {
    id: t("entityGrid.id"),
    type: t("entityGrid.type"),
    createdAt: t("entityGrid.createdAt"),
    modifiedAt: t("entityGrid.modifiedAt"),
    observedAt: t("entityGrid.observedAt"),
    unit: t("entityGrid.unit"),
    datasetId: t("entityGrid.datasetId"),
    empty: t("entityGrid.empty"),
    loading: t("entityGrid.loading"),
    previous: t("entityGrid.previous"),
    next: t("entityGrid.next"),
    page: t("entityGrid.page"),
    showMetadata: t("entityGrid.showMetadata"),
    error: t("entityGrid.error"),
    filter: t("entityGrid.filter"),
    ops: {
      contains: t("entityGrid.ops.contains"),
      equals: t("entityGrid.ops.equals"),
      notEquals: t("entityGrid.ops.notEquals"),
      // The four comparisons are symbols in every language, so the SDK's own are kept.
      gt: ops.gt,
      gte: ops.gte,
      lt: ops.lt,
      lte: ops.lte,
      between: t("entityGrid.ops.between"),
      empty: t("entityGrid.ops.isEmpty"),
      present: t("entityGrid.ops.present"),
      pattern: t("entityGrid.ops.pattern"),
    },
    value: t("entityGrid.value"),
    upperValue: t("entityGrid.upperValue"),
    query: t("entityGrid.query"),
    copyQuery: t("entityGrid.copyQuery"),
    editAsText: t("entityGrid.editAsText"),
    filterRow: t("entityGrid.filterRow"),
    sortPage: t("entityGrid.sortPage"),
    matching: t("entityGrid.matching"),
    history: t("entityGrid.history"),
    historyLabels: {
      title: t("entityGrid.history"),
      window: {
        hour: t("entityGrid.historyWindow.hour"),
        day: t("entityGrid.historyWindow.day"),
        week: t("entityGrid.historyWindow.week"),
        custom: t("entityGrid.historyWindow.custom"),
      },
      from: t("entityGrid.from"),
      to: t("entityGrid.to"),
      at: t("entityGrid.observedAt"),
      value: t("entityGrid.value"),
      unit: t("entityGrid.unit"),
      empty: t("entityGrid.historyEmpty"),
      cut: t("entityGrid.historyCut"),
      copy: t("entityGrid.historyCopy"),
      close: t("entityGrid.close"),
      loading: t("entityGrid.loading"),
      error: t("entityGrid.error"),
    },
    edit: t("entityGrid.edit"),
    pending: t("entityGrid.pending"),
    review: t("entityGrid.review"),
    apply: t("entityGrid.apply"),
    discard: t("entityGrid.discard"),
    observedKeep: t("entityGrid.observedKeep"),
    observedNow: t("entityGrid.observedNow"),
    applying: t("entityGrid.applying"),
    refusedHere: t("entityGrid.refusedHere"),
  };
}

/** The geometry editor's words, for the editor the map panel opens (UI-72, T-1442). */
export function geoEditorLabels(t: (key: string) => string): GeoLabels {
  return {
    tools: t("entityGrid.geo.tools"),
    select: t("entityGrid.geo.select"),
    undo: t("entityGrid.geo.undo"),
    redo: t("entityGrid.geo.redo"),
    remove: t("entityGrid.geo.remove"),
    removeShape: t("entityGrid.geo.removeShape"),
    point: t("entityGrid.geo.point"),
    longitude: t("entityGrid.geo.longitude"),
    latitude: t("entityGrid.geo.latitude"),
    noGeometry: t("entityGrid.geo.noGeometry"),
    paste: t("entityGrid.geo.paste"),
    take: t("entityGrid.geo.take"),
    upload: t("entityGrid.geo.upload"),
    notJson: t("entityGrid.geo.notJson"),
    tooLarge: t("entityGrid.geo.tooLarge"),
    modes: {
      point: t("entityGrid.geo.modes.point"),
      linestring: t("entityGrid.geo.modes.linestring"),
      polygon: t("entityGrid.geo.modes.polygon"),
      rectangle: t("entityGrid.geo.modes.rectangle"),
      circle: t("entityGrid.geo.modes.circle"),
      freehand: t("entityGrid.geo.modes.freehand"),
    },
  };
}

/** The map panel's words, in the person's language (UI-72). */
export function gridMapLabels(t: (key: string) => string): GridMapLabels {
  return {
    panel: t("entityGrid.map.panel"),
    offTheMap: t("entityGrid.map.offTheMap"),
    drawArea: t("entityGrid.map.drawArea"),
    clearArea: t("entityGrid.map.clearArea"),
    areaAsked: t("entityGrid.map.areaAsked"),
    areaSimplified: t("entityGrid.map.areaSimplified"),
    editGeometry: t("entityGrid.map.editGeometry"),
    moved: t("entityGrid.map.moved"),
    reshaped: t("entityGrid.map.reshaped"),
    replaced: t("entityGrid.map.replaced"),
    noGeometry: t("entityGrid.map.noGeometry"),
    viewerCannotEdit: t("entityGrid.map.viewerCannotEdit"),
  };
}

export interface PortalEntityGridProps extends Omit<EntityGridProps, "labels" | "source" | "state" | "onStateChange"> {
  /** The project whose layout this is, so two projects do not share one person's columns. */
  project: string;
  config: ResolvedGridConfig;
  /** A source to use instead of the one the config names, for a test or a fixture. */
  source?: EntityGridProps["source"];
}

/**
 * The grid as the Portal renders it. The source comes from the config — an endpoint's slug or a
 * space's name, never a URL (EP-55) — over the Portal's own origin transport.
 */
export function PortalEntityGrid({ project, config, source, ...rest }: PortalEntityGridProps): JSX.Element {
  const { t, i18n } = useTranslation();
  // `t` changes identity on a language change, which is what rebuilds the labels; the language
  // itself is read below, for the source's own locale.
  const labels = useMemo(() => gridLabels(t), [t]);
  const mapLabels = useMemo(() => gridMapLabels(t), [t]);
  const geoLabels = useMemo(() => geoEditorLabels(t), [t]);

  const built = useMemo(() => {
    if (source) {
      return source;
    }
    const where = config.source as GridSource;
    if (where.kind === "fixture") {
      throw new Error("a fixture source is for the SDK's own examples; pass `source` instead");
    }
    return sourceFor(where, originTransport(), i18n.language);
  }, [source, config.source, i18n.language]);

  const [layout, setLayout] = useState<Layout>(
    () => readLayout(project, config.type) ?? { shown: {}, sort: null },
  );
  const onStateChange = useCallback(
    (next: GridState) => {
      const kept: Layout = { shown: next.shown, sort: next.sort };
      setLayout(kept);
      writeLayout(project, config.type, kept);
    },
    [project, config.type],
  );

  return (
    <EntityGrid
      {...rest}
      config={config}
      source={built}
      labels={labels}
      // The map's own strings. No `mapEngine`: `GeoEditor`'s own default imports Terra Draw when a
      // map loads, so a page whose config asks for no map never downloads the library at all.
      mapLabels={mapLabels}
      geoLabels={geoLabels}
      // Only the layout is controlled from here: the offset, the filters and the edits stay the
      // grid's own, so nothing a person is in the middle of survives a reload it should not.
      state={layout}
      onStateChange={onStateChange}
    />
  );
}
