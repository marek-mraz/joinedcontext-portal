/**
 * Where an entity grid reads and writes (UI-71, SDK-29): an interface, two built-in sources over
 * the SDK transport, and an in-memory one for tests and examples.
 *
 * A source holds no credential: the transport carries the person's session and CSRF header as
 * every other SDK call does, so an Endpoint's Policy decides every read and write (EP-55). A config
 * names a slug or a space, never a URL, so these two path shapes are the only ones a grid reaches.
 */
import type { Transport } from "../sdk/transport";
import { queryString } from "../sdk/query";
import type { RichRow } from "./model";
import { toRichRow } from "./model";

export interface GridQuery {
  type: string;
  q?: string;
  attrs?: string[];
  idPattern?: string;
  scopeQ?: string;
}

export interface GridPage {
  rows: RichRow[];
  /** Known only where the source can count; the grid then pages by whether a page came back full. */
  total?: number;
}

export interface HistoryPoint {
  at: string;
  value: unknown;
}

export interface HistoryWindow {
  from?: string;
  to?: string;
  lastN?: number;
}

/** What a grid can do follows what its source offers: an absent capability switches its feature off. */
export interface EntitySource {
  query(q: GridQuery, page: { offset: number; limit: number }): Promise<GridPage>;
  get(id: string): Promise<RichRow | null>;
  history?(id: string, attr: string, window: HistoryWindow): Promise<HistoryPoint[]>;
  patch?(id: string, attrs: Record<string, unknown>): Promise<void>;
  remove?(id: string): Promise<void>;
}

export class SourceError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "SourceError";
    this.status = status;
  }
}

/** The problem document's own sentence, or `fallback`. */
function detailOf(body: unknown, fallback: string): string {
  if (typeof body === "object" && body !== null) {
    const problem = body as { detail?: unknown; title?: unknown };
    if (typeof problem.detail === "string" && problem.detail) return problem.detail;
    if (typeof problem.title === "string" && problem.title) return problem.title;
  }
  return fallback;
}

function entitiesQuery(q: GridQuery, offset: number, limit: number): string {
  // id and type come with every entity; asking for them as attributes is a 400.
  const attrs = (q.attrs ?? []).filter((attr) => attr !== "id" && attr !== "type");
  return queryString({
    type: q.type,
    options: "sysAttrs",
    limit: String(limit),
    offset: offset > 0 ? String(offset) : undefined,
    q: q.q || undefined,
    attrs: attrs.length > 0 ? attrs.join(",") : undefined,
    idPattern: q.idPattern || undefined,
    scopeQ: q.scopeQ || undefined,
  });
}

function historyQuery(attr: string, window: HistoryWindow): string {
  const { from, to, lastN } = window;
  return queryString({
    attrs: attr,
    timerel: from && to ? "between" : from ? "after" : to ? "before" : undefined,
    timeAt: from ?? to,
    endTimeAt: from && to ? to : undefined,
    lastN: lastN !== undefined ? String(lastN) : undefined,
  });
}

/** The instances of one attribute in a temporal answer: `[{value, observedAt}]`, or `{values: [[v, t]]}`. */
export function historyOf(body: unknown, attr: string): HistoryPoint[] {
  const attribute = typeof body === "object" && body !== null ? (body as Record<string, unknown>)[attr] : undefined;
  if (Array.isArray(attribute)) {
    return attribute.flatMap((instance) => {
      const at = (instance as { observedAt?: unknown })?.observedAt;
      return typeof at === "string" ? [{ at, value: (instance as { value?: unknown }).value }] : [];
    });
  }
  const values = (attribute as { values?: unknown } | undefined)?.values;
  if (Array.isArray(values)) {
    return values.flatMap((pair) =>
      Array.isArray(pair) && typeof pair[1] === "string" ? [{ at: pair[1], value: pair[0] }] : [],
    );
  }
  return [];
}

/**
 * One NGSI-LD API under `base`. `hidden` is what a 404 becomes on a query, a history read or a
 * write: a Context Space surface says the same whether the space is missing or not granted.
 */
function ngsiSource(base: string, transport: Transport, language: string, hidden?: string): EntitySource {
  const refuse = (status: number, body: unknown, fallback: string): never => {
    if (status === 404 && hidden) throw new SourceError(404, hidden);
    throw new SourceError(status, detailOf(body, fallback));
  };
  const ok = (status: number) => status >= 200 && status < 300;
  const entity = (id: string) => `${base}/entities/${encodeURIComponent(id)}`;

  return {
    async query(q, page) {
      const { status, body } = await transport({
        method: "GET",
        path: `${base}/entities?${entitiesQuery(q, page.offset, page.limit)}`,
      });
      if (!ok(status)) refuse(status, body, "the read was refused");
      if (!Array.isArray(body)) throw new SourceError(status, "the answer is not a list of entities");
      return { rows: body.map((item) => toRichRow(item as Record<string, unknown>, language)) };
    },
    async get(id) {
      const { status, body } = await transport({ method: "GET", path: `${entity(id)}?options=sysAttrs` });
      if (status === 404) return null;
      if (!ok(status)) refuse(status, body, "the read was refused");
      return toRichRow(body as Record<string, unknown>, language);
    },
    async history(id, attr, window) {
      const { status, body } = await transport({
        method: "GET",
        path: `${base}/temporal/entities/${encodeURIComponent(id)}?${historyQuery(attr, window)}`,
      });
      if (!ok(status)) refuse(status, body, "the history was refused");
      return historyOf(body, attr);
    },
    async patch(id, attrs) {
      const { status, body } = await transport({ method: "PATCH", path: `${entity(id)}/attrs`, body: attrs });
      if (!ok(status)) refuse(status, body, "the write was refused");
    },
    async remove(id) {
      const { status, body } = await transport({ method: "DELETE", path: entity(id) });
      if (!ok(status)) refuse(status, body, "the removal was refused");
    },
  };
}

/** What one Endpoint answers, by its slug (EP-55). */
export function endpointSource(slug: string, transport: Transport, language = "en"): EntitySource {
  return ngsiSource(`/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1`, transport, language);
}

/** A whole Context Space surface, by its segment (SP-06); a 404 never says whether the space exists. */
export function spaceSource(space: string, transport: Transport, language = "en"): EntitySource {
  return ngsiSource(`/cs/${encodeURIComponent(space)}/ngsi-ld/v1`, transport, language, "no grant on this space");
}

/** Entities in memory, for tests and examples: type and idPattern filter, paging, a total. */
export function fixtureSource(entities: Record<string, unknown>[], language = "en"): EntitySource {
  const rows = entities.map((entity) => toRichRow(entity, language));
  return {
    async query(q, page) {
      const pattern = q.idPattern ? new RegExp(q.idPattern) : null;
      const matching = rows.filter((row) => row.type === q.type && (!pattern || pattern.test(row.id)));
      return { rows: matching.slice(page.offset, page.offset + page.limit), total: matching.length };
    },
    async get(id) {
      return rows.find((row) => row.id === id) ?? null;
    },
  };
}

/** The built-in source a config names. */
export function sourceFor(
  config: { kind: "endpoint"; slug: string } | { kind: "space"; space: string },
  transport: Transport,
  language?: string,
): EntitySource {
  return config.kind === "endpoint"
    ? endpointSource(config.slug, transport, language)
    : spaceSource(config.space, transport, language);
}
