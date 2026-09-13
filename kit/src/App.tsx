import { useEffect, useMemo, useState } from "react";
import type { Cell, FilterState, Row } from "./ngsi";
import { applyFilters, columnKind, distinct, extent, loadSource, toRow } from "./ngsi";
import type { Filter, Source, Spec, View } from "./spec";
import { pagesOf, sourceOf } from "./spec";
import { Chart } from "./views/Chart";
import { Detail } from "./views/Detail";
import { Form } from "./views/Form";
import { MapView } from "./views/MapView";
import { Stats } from "./views/Stats";
import { Table } from "./views/Table";

const DEFAULT_ACCENT = "#0f766e";

type Loaded = Record<string, Row[]>;

/** Every source read once; `error` names the first that failed. */
/** Entities the Portal read for the preview and inlined, keyed by source name. */
export type Inline = Record<string, unknown[]>;

/**
 * Rows per source: from the document when the Portal inlined them, from the endpoint
 * otherwise. The preview frame is sandboxed without an origin, so it can fetch nothing with a
 * session; the published app is same-origin and reads live.
 */
export function useSources(slug: string, spec: Spec, inline?: Inline): { data: Loaded; loading: boolean; error: string | null } {
  const [data, setData] = useState<Loaded>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const load = async (source: Source): Promise<Row[]> => {
      const given = inline?.[source.name];
      if (Array.isArray(given)) {
        return given.filter((e): e is Record<string, unknown> => typeof e === "object" && e !== null).map(toRow);
      }
      return loadSource(slug, source);
    };
    Promise.all(spec.sources.map(async (source) => [source.name, await load(source)] as const))
      .then((pairs) => {
        if (!cancelled) setData(Object.fromEntries(pairs));
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [slug, spec, inline]);
  return { data, loading, error };
}

function FilterControl({ filter, index, rows, value, onChange }: { filter: Filter; index: number; rows: Row[]; value: FilterState[number]; onChange: (value: FilterState[number]) => void }) {
  const id = `filter-${index}`;
  if (filter.kind === "search") {
    return (
      <label className="filter" htmlFor={id}>
        <span>{filter.label ?? "Search"}</span>
        <input id={id} type="search" value={typeof value === "string" ? value : ""} placeholder={filter.attrs.join(", ")} onChange={(e) => onChange(e.target.value)} />
      </label>
    );
  }
  if (filter.kind === "select") {
    return (
      <label className="filter" htmlFor={id}>
        <span>{filter.label ?? filter.attr}</span>
        <select id={id} value={typeof value === "string" ? value : ""} onChange={(e) => onChange(e.target.value)}>
          <option value="">All</option>
          {distinct(rows, filter.attr).map((option) => (
            <option key={option} value={option}>{option}</option>
          ))}
        </select>
      </label>
    );
  }
  const range = extent(rows, filter.attr);
  if (!range || columnKind(rows, filter.attr) !== "number") {
    return null;
  }
  const current = Array.isArray(value) ? value : range;
  return (
    <div className="filter range" role="group" aria-label={filter.label ?? filter.attr}>
      <span>{filter.label ?? filter.attr}: {current[0]} – {current[1]}</span>
      <input type="range" min={range[0]} max={range[1]} step="any" value={current[0]} aria-label={`${filter.label ?? filter.attr} from`} onChange={(e) => onChange([Math.min(Number(e.target.value), current[1]), current[1]])} />
      <input type="range" min={range[0]} max={range[1]} step="any" value={current[1]} aria-label={`${filter.label ?? filter.attr} to`} onChange={(e) => onChange([current[0], Math.max(Number(e.target.value), current[0])])} />
    </div>
  );
}

function ViewCard({ view, spec, rows, accent, selected, onSelect, onSave }: { view: View; spec: Spec; rows: Row[]; accent: string; selected: string | null; onSelect: (id: string | null) => void; onSave: (id: string, patch: Record<string, Cell>) => void }) {
  const source = sourceOf(spec, view);
  const body = (() => {
    switch (view.kind) {
      case "stats":
        return <Stats rows={rows} items={view.items} />;
      case "map":
        return <MapView rows={rows} location={view.location ?? "location"} label={view.label} color={view.color} accent={accent} selected={selected} onSelect={onSelect} />;
      case "table":
        return <Table rows={rows} columns={view.columns} sort={view.sort} selected={selected} onSelect={onSelect} />;
      case "chart":
        return <Chart rows={rows} x={view.x} y={view.y} agg={view.agg} top={view.top} type={view.type} accent={accent} />;
      case "detail":
        return <Detail row={rows.find((r) => r.id === selected) ?? null} attrs={source.attrs} />;
      case "form":
        return <Form row={rows.find((r) => r.id === selected) ?? null} rows={rows} fields={view.fields ?? source.attrs} title={view.title} onSave={onSave} onClose={() => onSelect(null)} />;
      default:
        return null;
    }
  })();
  return (
    <section className={`card card-${view.kind}`} aria-label={view.title ?? view.kind}>
      {view.title && <h2>{view.title}</h2>}
      {body}
    </section>
  );
}

export function App({ slug, spec, inline }: { slug: string; spec: Spec; inline?: Inline }) {
  const { data, loading, error } = useSources(slug, spec, inline);
  const [state, setState] = useState<FilterState>({});
  const [selected, setSelected] = useState<string | null>(null);
  // What a form saved, by entity id, laid over the rows read: on screen only.
  const [edits, setEdits] = useState<Record<string, Record<string, Cell>>>({});
  const pages = pagesOf(spec);
  const [page, setPage] = useState<string>(pages[0] ?? "");
  const accent = spec.theme?.accent ?? DEFAULT_ACCENT;
  const filters = spec.filters ?? [];
  const edited = useMemo(
    () => Object.fromEntries(spec.sources.map((s) => [s.name, (data[s.name] ?? []).map((row) => (edits[row.id] ? { ...row, ...edits[row.id] } : row))])) as Loaded,
    [data, edits, spec.sources],
  );
  const filtered = useMemo(
    () => Object.fromEntries(spec.sources.map((s) => [s.name, applyFilters(edited[s.name] ?? [], filters, state, s.name)])) as Loaded,
    [edited, filters, state, spec.sources],
  );
  const save = (id: string, patch: Record<string, Cell>) => setEdits((e) => ({ ...e, [id]: { ...e[id], ...patch } }));
  const shown = spec.views.filter((view) => view.page === undefined || view.page === page);
  const total = spec.sources.reduce((n, s) => n + (data[s.name]?.length ?? 0), 0);
  const kept = spec.sources.reduce((n, s) => n + (filtered[s.name]?.length ?? 0), 0);

  return (
    <div className="app" style={{ ["--accent" as string]: accent }}>
      <header>
        <div>
          <h1>{spec.title}</h1>
          {spec.subtitle && <p className="subtitle">{spec.subtitle}</p>}
        </div>
        <span className="status" role="status">
          {loading ? "Loading…" : error ? error : kept === total ? `${total} entities` : `${kept} of ${total} entities`}
        </span>
      </header>
      {filters.length > 0 && (
        <div className="filters">
          {filters.map((filter, index) => (
            <FilterControl key={index} filter={filter} index={index} rows={data[sourceOf(spec, filter).name] ?? []} value={state[index]} onChange={(value) => setState((s) => ({ ...s, [index]: value }))} />
          ))}
        </div>
      )}
      {error && <p role="alert" className="error">{error}</p>}
      {pages.length > 0 && (
        <nav className="pages" aria-label="Pages">
          {pages.map((name) => (
            <button key={name} type="button" aria-current={name === page ? "page" : undefined} onClick={() => setPage(name)}>{name}</button>
          ))}
        </nav>
      )}
      <main>
        {shown.map((view) => (
          <ViewCard key={spec.views.indexOf(view)} view={view} spec={spec} rows={filtered[sourceOf(spec, view).name] ?? []} accent={accent} selected={selected} onSelect={setSelected} onSave={save} />
        ))}
      </main>
    </div>
  );
}
