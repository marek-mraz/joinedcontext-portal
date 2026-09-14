import type { Cell, Row } from "../ngsi";
import { cell, toRow } from "../ngsi";
import type { Schema } from "../write";
import type { AccessDocument } from "./access";
import { parseAccess } from "./access";
import type { JcConfig, JcEndpoint, JcUser } from "./config";
import { readConfig } from "./config";
import { queryString, randomId } from "./query";
import type { Transport } from "./transport";
import { transportFor } from "./transport";

export class ProblemError extends Error {
  readonly status: number;
  readonly title: string;
  readonly detail?: string;
  readonly type?: string;
  readonly file?: string;
  readonly line?: number;

  constructor(status: number, body: unknown) {
    const b = (typeof body === "object" && body !== null ? body : {}) as {
      title?: unknown;
      detail?: unknown;
      type?: unknown;
      error?: { message?: unknown; file?: unknown; line?: unknown };
    };

    const runtimeMsg = typeof b.error?.message === "string" ? b.error.message : undefined;
    const title = typeof b.title === "string" ? b.title : (runtimeMsg ?? (status ? `HTTP ${status}` : "Network Error"));
    const detail = typeof b.detail === "string" ? b.detail : runtimeMsg;
    const message = detail ?? title;

    super(message);
    this.name = "ProblemError";
    this.status = status;
    this.title = title;
    this.detail = detail;
    this.type = typeof b.type === "string" ? b.type : undefined;
    this.file = typeof b.error?.file === "string" ? b.error.file : undefined;
    this.line = typeof b.error?.line === "number" ? b.error.line : undefined;
  }
}

export interface Query {
  /** The endpoint to read, by the name the served configuration lists; only needed for a type more than one endpoint serves. */
  endpoint?: string;
  attrs?: string[];
  q?: string;
  georel?: string;
  geometry?: string;
  coordinates?: string;
  limit?: number;
  offset?: number;
}

export interface TemporalQuery {
  endpoint?: string;
  attrs?: string[];
  q?: string;
  timerel: "before" | "after" | "between";
  timeAt: string;
  endTimeAt?: string;
  lastN?: number;
  limit?: number;
}

export interface TemporalPoint {
  value: Cell;
  observedAt: string;
}

export interface TemporalRow {
  id: string;
  type: string;
  series: Record<string, TemporalPoint[]>;
}

export interface DataClient {
  readonly config: JcConfig;
  entities: {
    list<T extends Row = Row>(type: string, query?: Query): Promise<T[]>;
    all<T extends Row = Row>(type: string, query?: Query): Promise<T[]>;
    get<T extends Row = Row>(id: string, attrs?: string[], options?: EndpointOption): Promise<T>;
    create(type: string, attrs: Record<string, Cell>, localId?: string, options?: EndpointOption): Promise<string>;
    update(id: string, patch: Record<string, Cell>, options?: EndpointOption): Promise<void>;
    remove(id: string, options?: EndpointOption): Promise<void>;
  };
  temporal: { list(type: string, query: TemporalQuery): Promise<TemporalRow[]> };
  /** Every endpoint's schemas merged, or one endpoint's by name. */
  schema(endpoint?: string): Promise<Schema>;
  /** One endpoint's grant document, the primary's by default. */
  access(endpoint?: string): Promise<AccessDocument>;
  me(): JcUser | null;
  entityId(type: string, localId: string, options?: EndpointOption): string;
}

/** Names one endpoint of an application that reads several (SDK-02). */
export interface EndpointOption {
  endpoint?: string;
}

export interface Client extends DataClient {
  functions: { call<T = unknown>(name: string, body?: unknown): Promise<T> };
}

export const FUNCTION_NAME = /^[a-z][a-z0-9-]{0,39}$/;
const TYPE_RE = /^[A-Za-z][A-Za-z0-9_]*$/;
const LOCAL_ID_RE = /^[A-Za-z0-9._~-]{1,128}$/;

export function isEndpointPath(slug: string, path: string): boolean {
  if (path.includes("..") || path.includes("//") || path.includes("\\") || path.includes("#")) {
    return false;
  }
  const prefix = `/api/endpoint/${encodeURIComponent(slug)}/`;
  if (!path.startsWith(prefix)) {
    return false;
  }
  const rest = path.slice(prefix.length);
  const [pathname, ...query] = rest.split("?");
  if (query.length > 1) return false;

  const valid =
    /^(ngsi-ld\/v1\/entities(\/[^/]+(\/attrs)?)?|ngsi-ld\/v1\/temporal\/entities|schema\/index\.json|schema\/v\d+\/json-schema|access)$/;
  return valid.test(pathname);
}

function checkEndpointPath(slug: string, path: string): void {
  if (!isEndpointPath(slug, path)) {
    throw new ProblemError(0, { title: `Refused unauthorized endpoint path: ${path}` });
  }
}

function isGeoJsonGeometry(value: unknown): value is { type: string; coordinates: unknown } {
  return typeof value === "object" && value !== null && "type" in value && "coordinates" in value;
}

function encodeAttrs(attrs: Record<string, Cell>): Record<string, { type: "Property" | "GeoProperty"; value: Cell }> {
  const result: Record<string, { type: "Property" | "GeoProperty"; value: Cell }> = {};
  for (const [key, val] of Object.entries(attrs)) {
    if (val === null) continue;
    if (isGeoJsonGeometry(val)) {
      result[key] = { type: "GeoProperty", value: val };
    } else {
      result[key] = { type: "Property", value: val };
    }
  }
  return result;
}

/** The endpoints a configuration names; one that names none reads its one endpoint. */
export function endpointsOf(config: JcConfig): JcEndpoint[] {
  if (config.endpoints && config.endpoints.length > 0) {
    return config.endpoints;
  }
  return [{ name: config.endpointName ?? config.slug, slug: config.slug, space: config.space, types: [] }];
}

/**
 * The endpoint a call reads (SDK-02): the named one; otherwise the one endpoint serving the
 * type; a type several serve is refused until the call names one; anything else the primary.
 */
export function resolveEndpoint(endpoints: JcEndpoint[], type?: string, name?: string): JcEndpoint {
  const names = endpoints.map((e) => e.name).join(", ");
  if (name !== undefined) {
    const named = endpoints.find((e) => e.name === name);
    if (!named) {
      throw new ProblemError(0, { title: `Unknown endpoint '${name}'`, detail: `Unknown endpoint '${name}': the application reads ${names}.` });
    }
    return named;
  }
  if (type !== undefined) {
    const serving = endpoints.filter((e) => e.types.includes(type));
    if (serving.length === 1) {
      return serving[0];
    }
    if (serving.length > 1) {
      throw new ProblemError(0, {
        title: `Type '${type}' is served by more than one endpoint`,
        detail: `Type '${type}' is served by ${serving.map((e) => e.name).join(" and ")}; pass { endpoint: "${serving[0].name}" } to say which one to read.`,
      });
    }
  }
  return endpoints[0];
}

export function createClient(config: JcConfig, transport: Transport): Client {
  const schemas = new Map<string, Schema>();
  const accesses = new Map<string, AccessDocument>();
  const known = endpointsOf(config);

  /** The endpoint of an entity id: its type, and its space when several endpoints serve the type. */
  const endpointOfId = (id: string, name?: string): JcEndpoint => {
    if (name !== undefined || known.length === 1) {
      return resolveEndpoint(known, undefined, name);
    }
    const parts = id.split(":");
    const type = parts[2];
    const space = parts[4];
    const serving = known.filter((e) => e.types.includes(type));
    const inSpace = serving.find((e) => e.space === space) ?? known.find((e) => e.space === space);
    return inSpace ?? resolveEndpoint(known, type);
  };

  const entityId = (type: string, localId: string, options?: EndpointOption): string => {
    const space = known.length === 1 ? config.space : resolveEndpoint(known, type, options?.endpoint).space || config.space;
    return `urn:ngsi-ld:${type}:${config.orgDomain}:${space}:${localId}`;
  };

  const entities = {
    async list<T extends Row = Row>(type: string, query?: Query): Promise<T[]> {
      if (!TYPE_RE.test(type)) {
        throw new ProblemError(0, { title: `Invalid entity type: '${type}'` });
      }
      const limit = query?.limit ?? 100;
      if (typeof limit !== "number" || limit < 1 || limit > 1000) {
        throw new ProblemError(0, { title: `limit must be between 1 and 1000, got ${limit}` });
      }
      if (query?.offset !== undefined && (typeof query.offset !== "number" || query.offset < 0)) {
        throw new ProblemError(0, { title: `offset must be >= 0, got ${query.offset}` });
      }

      const geo = [query?.georel, query?.geometry, query?.coordinates];
      const geoCount = geo.filter((g) => g !== undefined).length;
      if (geoCount > 0 && geoCount < 3) {
        throw new ProblemError(0, { title: "georel, geometry, coordinates must all be provided together" });
      }

      const params: Record<string, string | undefined> = { type, options: "keyValues", limit: String(limit) };
      if (query?.offset !== undefined && query.offset > 0) {
        params.offset = String(query.offset);
      }
      if (query?.attrs) {
        const filtered = query.attrs.filter((a) => a !== "id" && a !== "type" && a !== "@context");
        if (filtered.length > 0) {
          params.attrs = filtered.join(",");
        }
      }
      if (query?.q) {
        params.q = query.q;
      }
      if (query?.georel && query?.geometry && query?.coordinates) {
        params.georel = query.georel;
        params.geometry = query.geometry;
        params.coordinates = query.coordinates;
      }

      const { slug } = resolveEndpoint(known, type, query?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?${queryString(params)}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      if (!Array.isArray(resp.body)) {
        throw new ProblemError(resp.status, { title: "The endpoint did not answer a list." });
      }

      return resp.body.map((item) => toRow(item as Record<string, unknown>, config.language)) as T[];
    },

    async all<T extends Row = Row>(type: string, query?: Query): Promise<T[]> {
      const maxTotal = Math.min(query?.limit ?? 1000, 5000);
      const rows: T[] = [];
      let offset = query?.offset ?? 0;

      while (rows.length < maxTotal) {
        const pageLimit = Math.min(1000, maxTotal - rows.length);
        const page = await entities.list<T>(type, { ...query, limit: pageLimit, offset });
        rows.push(...page);
        if (page.length < pageLimit) {
          break;
        }
        offset += page.length;
      }

      return rows;
    },

    async get<T extends Row = Row>(id: string, attrs?: string[], options?: EndpointOption): Promise<T> {
      const params: Record<string, string | undefined> = { options: "keyValues" };
      if (attrs) {
        const filtered = attrs.filter((a) => a !== "id" && a !== "type" && a !== "@context");
        if (filtered.length > 0) params.attrs = filtered.join(",");
      }
      const qs = queryString(params);
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}${qs ? `?${qs}` : ""}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      if (typeof resp.body !== "object" || resp.body === null) {
        throw new ProblemError(resp.status, { title: "The endpoint did not answer an entity." });
      }
      return toRow(resp.body as Record<string, unknown>, config.language) as T;
    },

    async create(type: string, attrs: Record<string, Cell>, localId?: string, options?: EndpointOption): Promise<string> {
      if (!TYPE_RE.test(type)) {
        throw new ProblemError(0, { title: `Invalid entity type: '${type}'` });
      }
      const lid = localId ?? randomId();
      if (!LOCAL_ID_RE.test(lid)) {
        throw new ProblemError(0, { title: `Invalid localId: '${lid}'` });
      }
      const { slug } = resolveEndpoint(known, type, options?.endpoint);
      const id = entityId(type, lid, options);
      const body = {
        id,
        type,
        ...encodeAttrs(attrs),
      };

      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "POST", path, body });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      return id;
    },

    async update(id: string, patch: Record<string, Cell>, options?: EndpointOption): Promise<void> {
      const encoded = encodeAttrs(patch);
      if (Object.keys(encoded).length === 0) {
        return;
      }
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}/attrs`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "PATCH", path, body: encoded });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
    },

    async remove(id: string, options?: EndpointOption): Promise<void> {
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "DELETE", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
    },
  };

  const temporal = {
    async list(type: string, query: TemporalQuery): Promise<TemporalRow[]> {
      if (!TYPE_RE.test(type)) {
        throw new ProblemError(0, { title: `Invalid entity type: '${type}'` });
      }
      if (query.timerel === "between" && !query.endTimeAt) {
        throw new ProblemError(0, { title: "endTimeAt is required for timerel 'between'" });
      }

      const params: Record<string, string | undefined> = {
        type,
        options: "temporalValues",
        timerel: query.timerel,
        timeAt: query.timeAt,
      };
      if (query.endTimeAt) params.endTimeAt = query.endTimeAt;
      if (query.lastN !== undefined) params.lastN = String(query.lastN);
      if (query.limit !== undefined) params.limit = String(query.limit);
      if (query.q) params.q = query.q;
      if (query.attrs) {
        const filtered = query.attrs.filter((a) => a !== "id" && a !== "type" && a !== "@context");
        if (filtered.length > 0) params.attrs = filtered.join(",");
      }

      const { slug } = resolveEndpoint(known, type, query.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/temporal/entities?${queryString(params)}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      if (!Array.isArray(resp.body)) {
        throw new ProblemError(resp.status, { title: "The endpoint did not answer a list." });
      }

      return resp.body.map((item: unknown) => {
        const obj = (typeof item === "object" && item !== null ? item : {}) as Record<string, unknown>;
        const series: Record<string, TemporalPoint[]> = {};

        for (const [key, val] of Object.entries(obj)) {
          if (key === "id" || key === "type" || key === "@context") continue;
          if (typeof val === "object" && val !== null) {
            const rawProp = val as { values?: unknown[] };
            const values = Array.isArray(rawProp.values) ? rawProp.values : Array.isArray(val) ? val : [];
            series[key] = values
              .map((entry): TemporalPoint | null => {
                if (Array.isArray(entry) && entry.length >= 2) {
                  return { value: cell(entry[0], config.language), observedAt: String(entry[1]) };
                }
                if (typeof entry === "object" && entry !== null && "value" in entry && "observedAt" in entry) {
                  const e = entry as { value: unknown; observedAt: unknown };
                  return { value: cell(e.value, config.language), observedAt: String(e.observedAt) };
                }
                return null;
              })
              .filter((p): p is TemporalPoint => p !== null);
          }
        }

        return {
          id: String(obj.id ?? ""),
          type: String(obj.type ?? ""),
          series,
        };
      });
    },
  };

  const schema = async (endpoint?: string): Promise<Schema> => {
    if (endpoint === undefined && known.length > 1) {
      // Every endpoint's schema, merged; a type two endpoints serve keeps the first one's.
      const merged: Schema = {};
      for (const e of [...known].reverse()) {
        Object.assign(merged, await schema(e.name));
      }
      return merged;
    }
    const { slug, name } = resolveEndpoint(known, undefined, endpoint);
    const cachedSchema = schemas.get(name);
    if (cachedSchema) return cachedSchema;

    const indexPath = `/api/endpoint/${encodeURIComponent(slug)}/schema/index.json`;
    checkEndpointPath(slug, indexPath);
    const indexResp = await transport({ method: "GET", path: indexPath });
    if (indexResp.status < 200 || indexResp.status >= 300) {
      throw new ProblemError(indexResp.status, indexResp.body);
    }
    const indexBody = (typeof indexResp.body === "object" && indexResp.body !== null ? indexResp.body : {}) as {
      models?: Array<{ version?: number; types?: unknown }>;
    };
    // An endpoint may expose several models; their types never overlap, so the schemas merge.
    const versions = [...new Set((indexBody.models ?? []).map((m) => m.version).filter((v): v is number => Number.isInteger(v)))];
    const merged: Schema = {};
    for (const version of versions.length > 0 ? versions : [1]) {
      const schemaPath = `/api/endpoint/${encodeURIComponent(slug)}/schema/v${version}/json-schema`;
      checkEndpointPath(slug, schemaPath);
      const schemaResp = await transport({ method: "GET", path: schemaPath });
      if (schemaResp.status < 200 || schemaResp.status >= 300) {
        throw new ProblemError(schemaResp.status, schemaResp.body);
      }
      const schemaDoc = (typeof schemaResp.body === "object" && schemaResp.body !== null ? schemaResp.body : {}) as {
        definitions?: Schema;
        $defs?: Schema;
      };
      // Model Tools renders draft-07 (`definitions`); a derived schema may use 2019-09 (`$defs`).
      Object.assign(merged, schemaDoc.$defs, schemaDoc.definitions);
    }
    // A schema document also defines the abstract base class and whatever the endpoint does not
    // serve; the index names the types there are rows of, so only those are the app's types.
    const served = (indexBody.models ?? []).flatMap((m) =>
      Array.isArray(m.types) ? m.types.filter((t): t is string => typeof t === "string") : [],
    );
    for (const name of Object.keys(merged)) {
      if (name === "Entity" || (served.length > 0 && !served.includes(name))) {
        delete merged[name];
      }
    }
    schemas.set(name, merged);
    return merged;
  };

  const access = async (endpoint?: string): Promise<AccessDocument> => {
    const { slug, name } = resolveEndpoint(known, undefined, endpoint);
    const cachedAccess = accesses.get(name);
    if (cachedAccess) return cachedAccess;

    const path = `/api/endpoint/${encodeURIComponent(slug)}/access`;
    checkEndpointPath(slug, path);
    const resp = await transport({ method: "GET", path });
    if (resp.status < 200 || resp.status >= 300) {
      throw new ProblemError(resp.status, resp.body);
    }
    const parsed = parseAccess(resp.body);
    accesses.set(name, parsed);
    return parsed;
  };

  const functions = {
    async call<T = unknown>(name: string, body?: unknown): Promise<T> {
      if (!FUNCTION_NAME.test(name)) {
        throw new ProblemError(0, { title: `Invalid function name: '${name}'` });
      }

      const path =
        config.transport === "bridge"
          ? `/functions/${name}`
          : `/apps/${config.appName ?? ""}/api/functions/${name}`;

      const resp = await transport({ method: "POST", path, body });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      return resp.body as T;
    },
  };

  return {
    config,
    entities,
    temporal,
    schema,
    access,
    me: () => config.user ?? null,
    entityId,
    functions,
  };
}

let activeClient: Client | undefined;

export function jc(): Client {
  if (!activeClient) {
    const config = readConfig();
    activeClient = createClient(config, transportFor(config));
  }
  return activeClient;
}

export function setClient(client?: Client): void {
  activeClient = client;
}
