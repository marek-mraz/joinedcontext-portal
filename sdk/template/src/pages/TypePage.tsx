import { useMemo, useState } from "react";
import {
  BarChartCard,
  DateRangeFilter,
  EntityDetail,
  EntityForm,
  EntityMap,
  EntityTable,
  ExportButton,
  FilterBar,
  RangeFilter,
  SearchBox,
  SelectFilter,
  StatTiles,
  TimeSeriesCard,
  useAccess,
  useEntities,
  useFilters,
} from "@joinedcontext/sdk";
import type { FilterBinding, StatTile, TypeSchema } from "@joinedcontext/sdk";
import { filtersOf, shapeOf } from "./shape";

function Filter({ binding }: { binding: FilterBinding }) {
  switch (binding.def.kind) {
    case "search":
      return <SearchBox binding={binding} />;
    case "select":
      return <SelectFilter binding={binding} />;
    case "range":
      return <RangeFilter binding={binding} />;
    case "dateRange":
      return <DateRangeFilter binding={binding} />;
  }
}

/** One entity type: filters, tiles, map, charts, table, export, detail and, where granted, an edit form. */
export function TypePage({ type, schema }: { type: string; schema?: TypeSchema | null }) {
  const { rows, loading, error, reload } = useEntities(type);
  // The shape is read once the first rows arrive, so the filters keep their positions on a reload.
  const [firstRows, setFirstRows] = useState(rows);
  if (firstRows.length === 0 && rows.length > 0) setFirstRows(rows);
  const shape = useMemo(() => shapeOf(schema, firstRows), [schema, firstRows]);
  const filters = useMemo(() => filtersOf(shape), [shape]);
  const { shown, bind, reset } = useFilters(rows, filters);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const { can } = useAccess();
  const edit = can("updateAttrs", type).ok ? can("updateAttrs", type) : can("updateEntity", type);
  const selected = shown.find((row) => row.id === selectedId) ?? null;
  const select = (id: string) => {
    setSelectedId(id);
    setEditing(false);
  };

  const measure = shape.numbers[0];
  const tiles: StatTile[] = [
    { label: type, agg: "count" },
    ...shape.numbers.slice(0, 3).map((attr): StatTile => ({ label: `Average ${attr}`, agg: "avg", attr })),
  ];

  return (
    <section className="app-page" aria-label={type}>
      <FilterBar shown={shown.length} total={rows.length} onReset={reset}>
        {filters.map((_, index) => (
          <Filter key={index} binding={bind(index)} />
        ))}
      </FilterBar>
      <StatTiles rows={shown} tiles={tiles} loading={loading} />
      <div className="app-grid">
        {shape.geo && (
          <EntityMap rows={shown} location={shape.geo} label={shape.label} color={measure} selected={selectedId} onSelect={(row) => select(row.id)} />
        )}
        {shape.categories[0] && (
          <BarChartCard
            rows={shown}
            x={shape.categories[0]}
            y={measure}
            agg={measure ? "avg" : "count"}
            title={measure ? `Average ${measure} by ${shape.categories[0]}` : `${type} by ${shape.categories[0]}`}
          />
        )}
        {shape.time && (
          <TimeSeriesCard rows={shown} time={shape.time} y={measure} title={measure ? `${measure} over time` : `${type} over time`} />
        )}
      </div>
      <EntityTable rows={shown} loading={loading} error={error} selected={selectedId} onSelect={(row) => select(row.id)} caption={type} />
      <ExportButton rows={shown} filename={type} formats={shape.geo ? ["csv", "geojson", "pdf"] : ["csv", "pdf"]} location={shape.geo} />
      {selected && !editing && (
        <div className="app-detail">
          <EntityDetail row={selected} title={shape.label ? String(selected[shape.label] ?? selected.id) : selected.id} onClose={() => setSelectedId(null)} />
          {edit.ok && (
            <button type="button" onClick={() => setEditing(true)}>
              Edit
            </button>
          )}
        </div>
      )}
      {selected && editing && (
        <EntityForm
          type={type}
          row={selected}
          rows={rows}
          onSaved={() => {
            setEditing(false);
            reload();
          }}
          onCancel={() => setEditing(false)}
        />
      )}
    </section>
  );
}
