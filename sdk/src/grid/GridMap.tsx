/**
 * The map beside the rows (UI-72): one panel, the same state as the grid.
 *
 * Not a second view of a second query. The panel draws the rows of the page that hold a geometry,
 * a click on a shape activates that row and the active row's shape is drawn as chosen, and in edit
 * mode the geometry of the active row is edited like any other cell — the change lands in the
 * grid's pending edits (UI-67) and is applied through the Endpoint by the same `apply.ts`, with the
 * person's session and the Endpoint's Policy deciding it (EP-55).
 *
 * The drawn area is the one filter the filter row cannot hold: a rectangle becomes `georel=within`
 * on the grid's query and stays visible until it is removed.
 */

import React, { useMemo, useState } from "react";

import { GeoEditor } from "../geo/GeoEditor";
import type { DrawEngine, GeoLabels } from "../geo/GeoEditor";
import { GeoView } from "../geo/GeoView";
import type { GeoFeature } from "../geo/GeoView";
import type { Geometry, GeometryType } from "../geo/validate";
import { areaQuery, ringOfBounds } from "./geoarea";
import type { GeoArea, GeoQuery } from "./geoarea";
import { diffGeometry, featuresOfRows, geometryOfRow, rowsOffTheMap } from "./mapRows";
import type { RichRow } from "./model";

/** Every string the panel shows, so a host translates it one key at a time. */
export interface GridMapLabels {
  panel: string;
  offTheMap: string;
  drawArea: string;
  clearArea: string;
  areaAsked: string;
  areaSimplified: string;
  editGeometry: string;
  moved: string;
  reshaped: string;
  replaced: string;
  noGeometry: string;
  viewerCannotEdit: string;
}

export const DEFAULT_GRID_MAP_LABELS: GridMapLabels = {
  panel: "Map",
  offTheMap: "not on the map",
  drawArea: "Draw area",
  clearArea: "Clear area",
  areaAsked: "Filtered to the drawn area",
  areaSimplified: "The filter is a simplified outline of what you drew",
  editGeometry: "Edit geometry",
  moved: "moved",
  reshaped: "reshaped",
  replaced: "replaced",
  noGeometry: "This row has no geometry yet",
  viewerCannotEdit: "You may look at this map, not change it",
};

/** "Point moved 38 m", "Polygon: 2 vertices added, area +4 %", in the host's own words. */
export function summaryOf(before: Geometry | null, after: Geometry | null, labels: GridMapLabels): string {
  const diff = diffGeometry(before, after);
  switch (diff.kind) {
    case "same":
      return "";
    case "moved": {
      const metres = diff.metres ?? 0;
      const shown = metres < 10 ? metres.toFixed(1) : String(Math.round(metres));
      return `${after?.type ?? ""} ${labels.moved} ${shown} m`.trim();
    }
    case "reshaped": {
      const vertices = diff.vertices ?? 0;
      const parts: string[] = [];
      if (vertices !== 0) {
        const word = Math.abs(vertices) === 1 ? "vertex" : "vertices";
        parts.push(`${Math.abs(vertices)} ${word} ${vertices > 0 ? "added" : "removed"}`);
      }
      if (diff.areaPercent !== undefined && diff.areaPercent !== 0) {
        parts.push(`area ${diff.areaPercent > 0 ? "+" : ""}${diff.areaPercent} %`);
      }
      const detail = parts.length > 0 ? `: ${parts.join(", ")}` : "";
      return `${after?.type ?? ""} ${labels.reshaped}${detail}`.trim();
    }
    default:
      return `${labels.replaced}: ${diff.from ?? labels.noGeometry} → ${diff.to ?? labels.noGeometry}`;
  }
}

export interface GridMapProps {
  rows: RichRow[];
  /** The attribute this panel is about; the host resolved it with `mapAttrOf`. */
  attr: string;
  /** The row the grid has active, and how to move it when a shape is clicked. */
  activeId: string | null;
  onActivate: (id: string) => void;
  /** Pending cell edits, exactly as the grid holds them (UI-67). */
  edits?: Record<string, Record<string, unknown>>;
  /** A geometry a person changed; `undefined` gives the endpoint's value back. */
  onEdit?: (id: string, attr: string, value: unknown | undefined) => void;
  /** `edit` offers the editor for the active row; `view` never does. */
  mode?: "view" | "edit";
  /** The geometries this type accepts, from the DataModel's range where the space has one. */
  allowed?: readonly GeometryType[];
  /** The area the grid is filtered to, and how to set or clear it. */
  area?: GeoArea | null;
  onArea?: (area: GeoArea | null) => void;
  /** The current map bounds, for the rectangle the "Draw area" action asks for. */
  boundsNow?: () => [number, number, number, number] | null;
  labels?: Partial<GridMapLabels>;
  basemap?: string;
  /** The drawing library, injected the same way `GeoEditor` takes it. */
  engine?: DrawEngine;
  geoLabels?: Partial<GeoLabels>;
  position?: "right" | "bottom";
}

export function GridMap({
  rows,
  attr,
  activeId,
  onActivate,
  edits,
  onEdit,
  mode = "view",
  allowed,
  area,
  onArea,
  boundsNow,
  labels: labelsPartial,
  basemap,
  engine,
  geoLabels,
  position = "right",
}: GridMapProps): React.JSX.Element {
  const labels = { ...DEFAULT_GRID_MAP_LABELS, ...labelsPartial };
  const [editing, setEditing] = useState(false);

  const features = useMemo(() => featuresOfRows(rows, attr, edits), [rows, attr, edits]);
  const offTheMap = useMemo(() => rowsOffTheMap(rows, attr, edits), [rows, attr, edits]);
  const activeRow = useMemo(() => rows.find((row) => row.id === activeId) ?? null, [rows, activeId]);

  // The endpoint's geometry for the active row, and the pending one if there is one: the summary is
  // the difference between exactly those two.
  const stored = activeRow ? geometryOfRow(activeRow, attr) : null;
  const pending = activeRow ? (edits?.[activeRow.id]?.[attr] as Geometry | undefined) : undefined;
  const shown = pending ?? stored;
  const summary = pending !== undefined ? summaryOf(stored, pending ?? null, labels) : "";

  // The drawn area as the query it will become, so the panel can say when the filter stopped being
  // the shape the person drew.
  const asked: GeoQuery | null = areaQuery(area ?? null);

  const canEdit = mode === "edit" && Boolean(onEdit);
  const editorOpen = canEdit && editing && activeRow !== null;

  const drawArea = (): void => {
    if (!onArea || !boundsNow) {
      return;
    }
    const bounds = boundsNow();
    if (!bounds) {
      return;
    }
    const [west, south, east, north] = bounds;
    onArea({ geoproperty: attr, geometry: { type: "Polygon", coordinates: [ringOfBounds(west, south, east, north)] } as Geometry });
  };

  return (
    <section className={`jc-grid-map jc-grid-map--${position}`} aria-label={labels.panel}>
      <header className="jc-grid-map__bar">
        <h3 className="jc-grid-map__title">{labels.panel}</h3>
        {onArea && boundsNow ? (
          <button type="button" className="jc-grid-map__action" onClick={area ? () => onArea(null) : drawArea}>
            {area ? labels.clearArea : labels.drawArea}
          </button>
        ) : null}
        {canEdit && activeRow ? (
          <button
            type="button"
            className="jc-grid-map__action"
            aria-pressed={editorOpen}
            onClick={() => setEditing((open) => !open)}
          >
            {labels.editGeometry}
          </button>
        ) : null}
      </header>

      {asked ? (
        <p className="jc-grid-map__area" role="status">
          {labels.areaAsked}
          {asked.simplified ? ` — ${labels.areaSimplified}` : ""}
        </p>
      ) : null}

      {editorOpen ? (
        <>
          <GeoEditor
            value={shown}
            onChange={(geometry) => onEdit?.(activeRow.id, attr, geometry ?? undefined)}
            allowed={allowed}
            // Every other row of the page is drawn behind the edited one to draw against.
            snapTo={features.filter((feature: GeoFeature) => feature.id !== activeRow.id)}
            basemap={basemap}
            engine={engine}
            labels={geoLabels}
            label={`${labels.editGeometry}: ${activeRow.id}`}
          />
          {summary ? (
            <p className="jc-grid-map__summary" role="status">
              {summary}
            </p>
          ) : null}
        </>
      ) : (
        <GeoView
          value={features}
          selectedId={activeId}
          onSelect={onActivate}
          basemap={basemap}
          label={labels.panel}
        />
      )}

      {mode === "edit" && !onEdit ? <p className="jc-grid-map__note">{labels.viewerCannotEdit}</p> : null}

      {offTheMap.length > 0 ? (
        <p className="jc-grid-map__off">
          {labels.offTheMap}: {offTheMap.length}
        </p>
      ) : null}
    </section>
  );
}
