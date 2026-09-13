/**
 * The one file the model writes: what the dashboard shows, over which entity types, with which
 * filters. Mirrors `agents/kit.rs` in the Portal, which is the copy that refuses a bad
 * specification before it reaches a browser (AG-54); this copy is what lets the bundle say
 * exactly what is wrong instead of rendering nothing when it is handed one anyway.
 */

export type Agg = "count" | "sum" | "avg" | "min" | "max";

export interface Source {
  /** A short name views and filters refer to. */
  name: string;
  /** The NGSI-LD entity type read from the endpoint. */
  type: string;
  /** The attributes the views may use; the request asks for these and no more. */
  attrs: string[];
  /** An NGSI-LD `q` applied on the endpoint, before any filter on screen. */
  q?: string;
  /** Entities read at most; default 1000, ceiling 5000. */
  limit?: number;
}

export type Filter =
  | { kind: "search"; source?: string; label?: string; attrs: string[] }
  | { kind: "select"; source?: string; label?: string; attr: string }
  | { kind: "range"; source?: string; label?: string; attr: string };

export interface StatItem {
  label: string;
  agg: Agg;
  attr?: string;
  unit?: string;
}

export type View =
  | { kind: "stats"; source?: string; title?: string; items: StatItem[] }
  | { kind: "map"; source?: string; title?: string; location?: string; label?: string; color?: string }
  | { kind: "table"; source?: string; title?: string; columns: string[]; sort?: { attr: string; dir: "asc" | "desc" } }
  | { kind: "chart"; source?: string; title?: string; type: "bar" | "line" | "pie"; x: string; y: string; agg?: Agg; top?: number }
  | { kind: "detail"; source?: string; title?: string };

export interface Spec {
  title: string;
  subtitle?: string;
  sources: Source[];
  filters?: Filter[];
  views: View[];
  theme?: { accent?: string };
}

export const DEFAULT_LIMIT = 1000;
export const MAX_LIMIT = 5000;
const AGGS: Agg[] = ["count", "sum", "avg", "min", "max"];
const FILTERS = ["search", "select", "range"];
const VIEWS = ["stats", "map", "table", "chart", "detail"];
const CHARTS = ["bar", "line", "pie"];
/** Two names every entity carries whatever the source asked for. */
const ALWAYS = ["id", "type"];

/** The parsed specification, or every reason it cannot be rendered. */
export function parseSpec(input: unknown): { spec: Spec; errors: [] } | { spec: null; errors: string[] } {
  const errors: string[] = [];
  const at = (path: string, message: string) => errors.push(`${path}: ${message}`);
  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    return { spec: null, errors: ["spec: must be an object"] };
  }
  const raw = input as Record<string, unknown>;
  if (typeof raw.title !== "string" || raw.title.trim() === "") {
    at("title", "must be a non-empty string");
  }
  const sources = Array.isArray(raw.sources) ? (raw.sources as unknown[]) : [];
  if (sources.length === 0) {
    at("sources", "must list at least one entity type");
  }
  const attrsOf = new Map<string, Set<string>>();
  sources.forEach((source, index) => {
    const s = (source ?? {}) as Record<string, unknown>;
    const path = `sources[${index}]`;
    if (typeof s.name !== "string" || s.name === "") {
      at(`${path}.name`, "must be a non-empty string");
    } else if (attrsOf.has(s.name)) {
      at(`${path}.name`, `'${s.name}' is used twice`);
    }
    if (typeof s.type !== "string" || s.type === "") {
      at(`${path}.type`, "must be an entity type");
    }
    const attrs = Array.isArray(s.attrs) ? s.attrs.filter((a): a is string => typeof a === "string") : [];
    if (attrs.length === 0) {
      at(`${path}.attrs`, "must list at least one attribute");
    }
    if (s.limit !== undefined && (typeof s.limit !== "number" || s.limit < 1 || s.limit > MAX_LIMIT)) {
      at(`${path}.limit`, `must be between 1 and ${MAX_LIMIT}`);
    }
    if (typeof s.name === "string" && !attrsOf.has(s.name)) {
      attrsOf.set(s.name, new Set([...ALWAYS, ...attrs]));
    }
  });
  const first = sources[0] && typeof (sources[0] as Record<string, unknown>).name === "string"
    ? ((sources[0] as Record<string, unknown>).name as string)
    : "";
  const resolve = (path: string, name: unknown): Set<string> | null => {
    const key = typeof name === "string" ? name : first;
    const known = attrsOf.get(key);
    if (!known) {
      at(`${path}.source`, `'${String(name ?? first)}' is not a source`);
      return null;
    }
    return known;
  };
  const check = (path: string, known: Set<string> | null, attr: unknown) => {
    if (typeof attr !== "string") {
      at(path, "must name an attribute");
    } else if (known && !known.has(attr)) {
      at(path, `'${attr}' is not among the source's attributes`);
    }
  };

  const filters = raw.filters === undefined ? [] : Array.isArray(raw.filters) ? (raw.filters as unknown[]) : null;
  if (filters === null) {
    at("filters", "must be a list");
  }
  (filters ?? []).forEach((filter, index) => {
    const f = (filter ?? {}) as Record<string, unknown>;
    const path = `filters[${index}]`;
    if (!FILTERS.includes(String(f.kind))) {
      at(`${path}.kind`, `must be one of ${FILTERS.join(", ")}`);
      return;
    }
    const known = resolve(path, f.source);
    if (f.kind === "search") {
      const attrs = Array.isArray(f.attrs) ? f.attrs : [];
      if (attrs.length === 0) {
        at(`${path}.attrs`, "must list the attributes to search");
      }
      attrs.forEach((attr, i) => check(`${path}.attrs[${i}]`, known, attr));
    } else {
      check(`${path}.attr`, known, f.attr);
    }
  });

  const views = Array.isArray(raw.views) ? (raw.views as unknown[]) : [];
  if (views.length === 0) {
    at("views", "must list at least one view");
  }
  views.forEach((view, index) => {
    const v = (view ?? {}) as Record<string, unknown>;
    const path = `views[${index}]`;
    if (!VIEWS.includes(String(v.kind))) {
      at(`${path}.kind`, `must be one of ${VIEWS.join(", ")}`);
      return;
    }
    const known = resolve(path, v.source);
    switch (v.kind) {
      case "stats": {
        const items = Array.isArray(v.items) ? (v.items as unknown[]) : [];
        if (items.length === 0) {
          at(`${path}.items`, "must list at least one tile");
        }
        items.forEach((item, i) => {
          const it = (item ?? {}) as Record<string, unknown>;
          if (!AGGS.includes(it.agg as Agg)) {
            at(`${path}.items[${i}].agg`, `must be one of ${AGGS.join(", ")}`);
          } else if (it.agg !== "count") {
            check(`${path}.items[${i}].attr`, known, it.attr);
          }
          if (typeof it.label !== "string") {
            at(`${path}.items[${i}].label`, "must be a string");
          }
        });
        break;
      }
      case "map":
        if (v.location !== undefined) check(`${path}.location`, known, v.location);
        if (v.label !== undefined) check(`${path}.label`, known, v.label);
        if (v.color !== undefined) check(`${path}.color`, known, v.color);
        break;
      case "table": {
        const columns = Array.isArray(v.columns) ? v.columns : [];
        if (columns.length === 0) {
          at(`${path}.columns`, "must list at least one column");
        }
        columns.forEach((c, i) => check(`${path}.columns[${i}]`, known, c));
        break;
      }
      case "chart":
        if (!CHARTS.includes(String(v.type))) {
          at(`${path}.type`, `must be one of ${CHARTS.join(", ")}`);
        }
        check(`${path}.x`, known, v.x);
        check(`${path}.y`, known, v.y);
        if (v.agg !== undefined && !AGGS.includes(v.agg as Agg)) {
          at(`${path}.agg`, `must be one of ${AGGS.join(", ")}`);
        }
        break;
      default:
        break;
    }
  });

  if (errors.length > 0) {
    return { spec: null, errors };
  }
  return { spec: raw as unknown as Spec, errors: [] };
}

/** The source a view or filter reads, the first one when it names none. */
export function sourceOf<T extends { source?: string }>(spec: Spec, item: T): Source {
  return spec.sources.find((s) => s.name === item.source) ?? spec.sources[0];
}
