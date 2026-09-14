import { useMemo } from "react";
import { BarChartCard, Loading, navigate, Problem, StatTiles, useEntities, useFunction } from "@joinedcontext/sdk";
import type { Schema, TypeSchema } from "@joinedcontext/sdk";
import type { Summary } from "../../functions/summary";
import { shapeOf } from "./shape";

function TypeCard({ type, schema }: { type: string; schema?: TypeSchema }) {
  const { rows, loading, error } = useEntities(type);
  const shape = useMemo(() => shapeOf(schema, rows), [schema, rows]);
  const measure = shape.numbers[0];
  return (
    <article className="app-card">
      <header>
        <h2>{type}</h2>
        <button type="button" onClick={() => navigate(type)}>
          Open
        </button>
      </header>
      <Problem error={error} />
      <StatTiles
        rows={rows}
        loading={loading}
        tiles={[{ label: "Entities", agg: "count" }, ...(measure ? [{ label: `Average ${measure}`, agg: "avg" as const, attr: measure }] : [])]}
      />
      {shape.categories[0] && <BarChartCard rows={rows} x={shape.categories[0]} y={measure} agg={measure ? "avg" : "count"} top={10} height={220} />}
    </article>
  );
}

/** A card per entity type from the rows, and the same counts computed by the `summary` function. */
export function Overview({ schema }: { schema: Schema }) {
  const types = Object.keys(schema).sort();
  const summary = useFunction<Summary>("summary", { types }, { enabled: types.length > 0 });
  return (
    <section className="app-page" aria-label="Overview">
      <div className="app-grid">
        {types.map((type) => (
          <TypeCard key={type} type={type} schema={schema[type]} />
        ))}
      </div>
      <article className="app-card" aria-label="Server summary">
        <h2>Server summary</h2>
        {summary.loading && <Loading />}
        <Problem error={summary.error} onRetry={summary.reload} />
        {summary.data && (
          <ul>
            {summary.data.types.map((item) => (
              <li key={item.type}>
                {item.type}: {item.count}
                {Object.entries(item.averages).map(([attr, value]) => ` · ${attr} ${value}`)}
              </li>
            ))}
          </ul>
        )}
      </article>
    </section>
  );
}
