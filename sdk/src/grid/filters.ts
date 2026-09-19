/**
 * The grid's filter row, as an NGSI-LD query (UI-66, EP-07): one filter per column, composed into
 * the `q` and the `idPattern` the endpoint answers.
 *
 * The endpoint does the filtering, never the browser: a grid shows one page of a set that can be
 * millions, so a filter applied to the loaded rows would answer about the page and look like an
 * answer about the set. Every operator here therefore has to be expressible in ETSI NGSI-LD
 * §4.9's query language, and the value has to survive the trip: a value is data, so it is quoted
 * and escaped, and a value that goes into a pattern (`~=`, `idPattern`) has its regular-expression
 * metacharacters escaped too — otherwise `.` in a name typed by a person matches everything, and a
 * `(` answers 400.
 */
import type { MetaKey } from "./useEntityGrid";

/** What a filter asks. `between` uses both values; `empty` and `present` use none. */
export type FilterOp =
  | "contains"
  | "equals"
  | "notEquals"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "between"
  | "empty"
  | "present"
  | "pattern";

/** One column's filter: the column's own key, what it asks, and the typed value(s). */
export interface ColumnFilter {
  op: FilterOp;
  value: string;
  /** The upper bound of `between`. */
  value2?: string;
}

/** What a column can be asked, by the kind of thing it holds. */
export type FilterKind = "text" | "number" | "date" | "relationship" | "id" | "geo" | "none";

const TEXT_OPS: FilterOp[] = ["contains", "equals", "notEquals", "empty", "present"];
const ORDERED_OPS: FilterOp[] = ["equals", "notEquals", "gt", "gte", "lt", "lte", "between", "empty", "present"];
const REF_OPS: FilterOp[] = ["equals", "notEquals", "empty", "present"];
const ID_OPS: FilterOp[] = ["pattern", "equals"];

/** The operators a column offers; `[]` for one that cannot be filtered at the endpoint. */
export function opsForKind(kind: FilterKind): FilterOp[] {
  switch (kind) {
    case "text":
      return [...TEXT_OPS];
    case "number":
    case "date":
      return [...ORDERED_OPS];
    case "relationship":
      return [...REF_OPS];
    case "id":
      return [...ID_OPS];
    // A geometry is filtered by the map's own geo query (georel), not by a typed value.
    default:
      return [];
  }
}

/** Whether the operator needs one value, two, or none. */
export function valuesNeeded(op: FilterOp): 0 | 1 | 2 {
  if (op === "empty" || op === "present") {
    return 0;
  }
  return op === "between" ? 2 : 1;
}

/** Whether a filter is ready to be sent: its operator has the values it needs. */
export function complete(filter: ColumnFilter | undefined): boolean {
  if (!filter) {
    return false;
  }
  const needed = valuesNeeded(filter.op);
  if (needed === 0) {
    return true;
  }
  if (filter.value.trim() === "") {
    return false;
  }
  return needed === 1 || (filter.value2 ?? "").trim() !== "";
}

const NUMBER = /^-?\d+(\.\d+)?$/;
const ISO = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?)?$/;

/** A value as a `q` literal: bare for a number, a boolean or a timestamp, quoted otherwise. */
function literal(raw: string): string {
  const value = raw.trim();
  if (value === "true" || value === "false" || NUMBER.test(value) || ISO.test(value)) {
    return value;
  }
  return `"${value.replace(/[\\"]/g, (c) => `\\${c}`)}"`;
}

/** A value inside a pattern: every regular-expression metacharacter stands for itself. */
export function quotePattern(raw: string): string {
  return raw.trim().replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
}

/**
 * The attribute a term names: the attribute itself, or one of its metadata members, which is the
 * same dotted path the NGSI-LD query language uses for an attribute's own properties
 * (`temperature.observedAt>=…`).
 */
export function term(attr: string, meta?: MetaKey | null): string {
  return meta ? `${attr}.${meta}` : attr;
}

/** One filter as a `q` term; `undefined` when it asks for nothing. */
function termOf(attr: string, meta: MetaKey | null | undefined, filter: ColumnFilter): string | undefined {
  if (!complete(filter)) {
    return undefined;
  }
  const path = term(attr, meta);
  switch (filter.op) {
    case "empty":
      return `!${path}`;
    case "present":
      return path;
    case "contains":
      return `${path}~="${quotePattern(filter.value)}"`;
    case "equals":
      return `${path}==${literal(filter.value)}`;
    case "notEquals":
      return `${path}!=${literal(filter.value)}`;
    case "gt":
      return `${path}>${literal(filter.value)}`;
    case "gte":
      return `${path}>=${literal(filter.value)}`;
    case "lt":
      return `${path}<${literal(filter.value)}`;
    case "lte":
      return `${path}<=${literal(filter.value)}`;
    case "between":
      return `${path}>=${literal(filter.value)};${path}<=${literal(filter.value2 ?? "")}`;
    // A pattern belongs to the id, which is not part of `q`.
    case "pattern":
      return undefined;
  }
}

/** A column a filter can name: the attribute, the metadata member, and what it holds. */
export interface FilterColumn {
  key: string;
  attr: string | null;
  meta?: MetaKey | null;
  kind: FilterKind;
}

/**
 * The query of a filter row: the `q` its terms compose (joined by `;`, which is NGSI-LD's "and")
 * and the `idPattern` the id column's own filter asks for. A filter that is not ready is left
 * out, so a row being typed never narrows the answer by accident.
 */
export function queryFromFilters(
  columns: FilterColumn[],
  filters: Record<string, ColumnFilter>,
): { q?: string; idPattern?: string } {
  const terms: string[] = [];
  let idPattern: string | undefined;
  for (const column of columns) {
    const filter = filters[column.key];
    if (!filter || !complete(filter)) {
      continue;
    }
    if (column.kind === "id") {
      // `idPattern` is a regular expression over the whole id; `equals` is one id, anchored.
      idPattern =
        filter.op === "equals" ? `^${quotePattern(filter.value)}$` : quotePattern(filter.value);
      continue;
    }
    if (!column.attr) {
      continue;
    }
    const built = termOf(column.attr, column.meta, filter);
    if (built) {
      terms.push(built);
    }
  }
  return { q: terms.length > 0 ? terms.join(";") : undefined, idPattern };
}

/** Two `q` strings as one: both must hold, which is what `;` means. */
export function andQ(...parts: (string | undefined)[]): string | undefined {
  const kept = parts.map((part) => part?.trim()).filter((part): part is string => Boolean(part));
  return kept.length > 0 ? kept.join(";") : undefined;
}
