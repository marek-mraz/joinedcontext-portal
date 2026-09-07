/**
 * Filters generated from a DataModel (UI-33): the slots of a LinkML class become filter rows,
 * the rows compose into the NGSI-LD `q` and parse back from it. The `q` string in the manifest
 * is the only truth; the rows are a view of it, so the explorer, the pipeline studio and the
 * YAML view cannot disagree about what a pipeline reads (PL-42).
 */
import { ApiError } from "../../api/client";
import type { Manifest } from "../../api/manifest";
import { endpointUrl } from "../../components/endpoints/links";
import { parseModel } from "../../pages/models/linkml";
import type { NgsiLdKind } from "../../pages/models/linkml";
import { parseResultsCount } from "../../pages/spaces/SpaceInside";

/** What the query reads: the four fields of `source.query` the manifest carries (PL-33). */
export interface EntityQuery {
  type?: string;
  attrs?: string[];
  q?: string;
  scopeQ?: string;
}

/** One attribute a filter row may name, typed by its LinkML slot when the model declares it. */
export interface FilterSlot {
  name: string;
  range?: string;
  kind: NgsiLdKind;
  /** The permissible values when the range is an enum of the model. */
  values?: string[];
  minimum?: number;
  maximum?: number;
}

export const OPS = ["==", "!=", ">", ">=", "<", "<=", "~=", "!~="] as const;
export type Op = (typeof OPS)[number];

export interface Filter {
  attribute: string;
  op: Op;
  value: string;
}

const NUMERIC = ["integer", "float", "double", "decimal"];
const TEMPORAL = ["date", "datetime"];
const RESERVED = ["id", "type", "@context"];

/** The slots of one class of an inline LinkML model, `[]` when the model is not inline. */
export function filterSlotsOf(model: Manifest | undefined, type: string | undefined): FilterSlot[] {
  const source = model?.spec.linkml ?? model?.spec.source;
  if (!type || typeof source !== "string" || !source.includes("\n")) {
    return [];
  }
  const parsed = parseModel(source);
  const cls = parsed.classes.find((c) => c.name === type);
  return (cls?.slots ?? [])
    .filter((name) => !RESERVED.includes(name))
    .map((name) => {
      const slot = parsed.slots.find((s) => s.name === name);
      const values = parsed.enums.find((e) => e.name === slot?.range)?.permissible_values;
      return {
        name,
        range: slot?.range,
        kind: slot?.kind ?? "Property",
        values: values?.map((v) => v.name),
        minimum: slot?.minimum_value,
        maximum: slot?.maximum_value,
      };
    });
}

/** The operators a slot's range makes sense with; everything for an attribute the model lacks. */
export function opsFor(slot: FilterSlot | undefined): Op[] {
  if (!slot) {
    return [...OPS];
  }
  if (slot.range === "boolean" || slot.values || slot.kind === "Relationship") {
    return ["==", "!="];
  }
  if (NUMERIC.includes(slot.range ?? "") || TEMPORAL.includes(slot.range ?? "")) {
    return ["==", "!=", ">", ">=", "<", "<="];
  }
  return ["==", "!=", "~=", "!~="];
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?)?$/;

/** Whether a value goes bare into `q` (number, boolean, date) or inside quotes. */
function bare(value: string, slot: FilterSlot | undefined): boolean {
  if (slot?.range === "boolean") {
    return true;
  }
  if (slot && (NUMERIC.includes(slot.range ?? "") || TEMPORAL.includes(slot.range ?? ""))) {
    return true;
  }
  if (slot) {
    return false;
  }
  return value === "true" || value === "false" || /^-?\d+(\.\d+)?$/.test(value) || ISO_DATE.test(value);
}

/** The `q` of the rows, ETSI NGSI-LD §4.9: `attr op value` joined by `;`; rows without a value are left out. */
export function qFromFilters(rows: Filter[], slots: FilterSlot[] = []): string {
  return rows
    .filter((row) => row.attribute.trim() !== "" && row.value.trim() !== "")
    .map((row) => {
      const slot = slots.find((s) => s.name === row.attribute);
      const value = row.value.trim();
      const literal = bare(value, slot) ? value : `"${value.replace(/[\\"]/g, (c) => `\\${c}`)}"`;
      return `${row.attribute.trim()}${row.op}${literal}`;
    })
    .join(";");
}

const TERM = /^([A-Za-z_][A-Za-z0-9_.]*(?:\[[^\]]+\])?)(==|!=|>=|<=|~=|!~=|>|<)(.+)$/;

/** The rows of a `q` in the simple grammar; `undefined` for `|`, brackets or anything else the rows cannot show. */
export function filtersFromQ(q: string | undefined): Filter[] | undefined {
  const text = (q ?? "").trim();
  if (text === "") {
    return [];
  }
  if (/[|()]/.test(text)) {
    return undefined;
  }
  const rows: Filter[] = [];
  for (const term of text.split(";")) {
    const match = TERM.exec(term.trim());
    if (!match) {
      return undefined;
    }
    const [, attribute, op, raw] = match;
    const value =
      raw.length >= 2 && raw.startsWith('"') && raw.endsWith('"')
        ? raw.slice(1, -1).replace(/\\(["\\])/g, "$1")
        : raw;
    rows.push({ attribute, op: op as Op, value });
  }
  return rows;
}

/** One entity as the gateway returns it, `keyValues` or normalized. */
export type Entity = Record<string, unknown> & { id: string };

export interface EntityPage {
  rows: Entity[];
  /** The broker's `NGSILD-Results-Count`, when it sent one. */
  count?: number;
}

function entities(body: unknown): Entity[] {
  return (Array.isArray(body) ? body : []).filter(
    (item): item is Entity =>
      typeof item === "object" && item !== null && typeof (item as Entity).id === "string",
  );
}

async function gatewayGet(slug: string, path: string): Promise<Response> {
  // A `Request` rather than a URL string, as `api/client.ts` sends: same origin, the user's session.
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, path), { headers: { Accept: "application/ld+json" } }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  return response;
}

/** One page of entities read through the endpoint with the signed-in session (UI-33). */
export async function fetchEntities(
  slug: string,
  query: EntityQuery,
  page: { limit: number; offset?: number; keyValues?: boolean; count?: boolean },
): Promise<EntityPage> {
  if (!query.type) {
    return { rows: [] };
  }
  const params = new URLSearchParams({ type: query.type, limit: String(page.limit) });
  if (page.offset) {
    params.set("offset", String(page.offset));
  }
  if (page.keyValues !== false) {
    params.set("options", "keyValues");
  }
  if (page.count) {
    params.set("count", "true");
  }
  if (query.q?.trim()) {
    params.set("q", query.q.trim());
  }
  if (query.scopeQ?.trim()) {
    params.set("scopeQ", query.scopeQ.trim());
  }
  if (query.attrs && query.attrs.length > 0) {
    params.set("attrs", query.attrs.join(","));
  }
  const response = await gatewayGet(slug, `/ngsi-ld/v1/entities?${params.toString()}`);
  return { rows: entities(await response.json()), count: parseResultsCount(response.headers) };
}

/** One entity in full (normalized), for the detail pane. */
export async function fetchEntity(slug: string, id: string): Promise<Entity> {
  const response = await gatewayGet(slug, `/ngsi-ld/v1/entities/${encodeURIComponent(id)}`);
  const body: unknown = await response.json();
  if (typeof body !== "object" || body === null || typeof (body as Entity).id !== "string") {
    throw new ApiError(502, "Not an entity");
  }
  return body as Entity;
}
