import type { Cell, Row } from "../ngsi";
import type { Schema } from "../write";
import type { AccessDocument } from "./access";
import type { Client, DataClient } from "./client";
import { createClient } from "./client";
import type { JcConfig, JcUser } from "./config";
import type { JcRequest, JcResponse, Transport } from "./transport";

export interface Fixture {
  entities?: Row[];
  schema?: Schema;
  access?: AccessDocument;
  functions?: Record<string, (body: unknown) => unknown | Promise<unknown>>;
  refuse?: (request: JcRequest) => JcResponse | null;
}

export interface StubTransport extends Transport {
  calls: JcRequest[];
  rows(): Row[];
}

function deriveSchema(rows: Row[]): Schema {
  const defs: Schema = {};
  for (const row of rows) {
    if (!defs[row.type]) {
      defs[row.type] = { properties: {} };
    }
    const props = defs[row.type].properties!;
    for (const [key, val] of Object.entries(row)) {
      if (key === "id" || key === "type" || key === "@context") continue;
      if (!props[key]) {
        let type = "string";
        if (typeof val === "number") type = "number";
        else if (typeof val === "boolean") type = "boolean";
        else if (typeof val === "object" && val !== null) type = "object";
        props[key] = { type };
      }
    }
  }
  return defs;
}

function decodeAttrs(body: unknown): Record<string, Cell> {
  const decoded: Record<string, Cell> = {};
  if (typeof body === "object" && body !== null) {
    for (const [k, v] of Object.entries(body as Record<string, unknown>)) {
      if (k === "id" || k === "type") continue;
      if (typeof v === "object" && v !== null && ("type" in v) && "value" in v) {
        decoded[k] = (v as { value: Cell }).value;
      } else {
        decoded[k] = v as Cell;
      }
    }
  }
  return decoded;
}

export function stubTransport(fixture?: Fixture): StubTransport {
  let store: Row[] = (fixture?.entities ?? []).map((r) => ({ ...r }));
  const calls: JcRequest[] = [];

  const transport = async (request: JcRequest): Promise<JcResponse> => {
    calls.push(request);
    if (fixture?.refuse) {
      const refused = fixture.refuse(request);
      if (refused) return refused;
    }

    const { method, path, body } = request;

    if (method === "POST" && path.startsWith("/functions/")) {
      const name = path.slice("/functions/".length);
      const fn = fixture?.functions?.[name];
      if (fn) {
        const res = await fn(body);
        return { status: 200, body: res };
      }
      return {
        status: 404,
        body: { type: "about:blank", title: "Not Found", status: 404, detail: `Function ${name} not found` },
      };
    }

    const [pathname, qs] = path.split("?");
    const params = new URLSearchParams(qs ?? "");

    if (pathname.endsWith("/ngsi-ld/v1/entities") && method === "GET") {
      const type = params.get("type");
      let items = type ? store.filter((r) => r.type === type) : [...store];
      const offset = Number(params.get("offset")) || 0;
      const limit = Number(params.get("limit")) || 100;
      const attrsStr = params.get("attrs");
      if (attrsStr) {
        const keep = new Set(["id", "type", ...attrsStr.split(",")]);
        items = items.map((r) => {
          const projected: Row = { id: r.id, type: r.type };
          for (const k of Object.keys(r)) {
            if (keep.has(k)) projected[k] = r[k];
          }
          return projected;
        });
      }
      return { status: 200, body: items.slice(offset, offset + limit) };
    }

    if (pathname.endsWith("/ngsi-ld/v1/entities") && method === "POST") {
      const raw = (body ?? {}) as Record<string, unknown>;
      const newRow: Row = {
        id: String(raw.id ?? ""),
        type: String(raw.type ?? ""),
        ...decodeAttrs(body),
      };
      store.push(newRow);
      return { status: 201, body: null };
    }

    if (pathname.includes("/ngsi-ld/v1/entities/") && pathname.endsWith("/attrs") && method === "PATCH") {
      const prefix = "/ngsi-ld/v1/entities/";
      const idx = pathname.indexOf(prefix);
      const idPart = pathname.slice(idx + prefix.length, pathname.length - "/attrs".length);
      const id = decodeURIComponent(idPart);
      const existing = store.find((r) => r.id === id);
      if (!existing) {
        return { status: 404, body: { title: "Entity not found", status: 404 } };
      }
      Object.assign(existing, decodeAttrs(body));
      return { status: 204, body: null };
    }

    if (pathname.includes("/ngsi-ld/v1/entities/") && method === "GET") {
      const prefix = "/ngsi-ld/v1/entities/";
      const id = decodeURIComponent(pathname.slice(pathname.indexOf(prefix) + prefix.length));
      const existing = store.find((r) => r.id === id);
      if (!existing) {
        return { status: 404, body: { title: "Entity not found", status: 404 } };
      }
      return { status: 200, body: existing };
    }

    if (pathname.includes("/ngsi-ld/v1/entities/") && method === "DELETE") {
      const prefix = "/ngsi-ld/v1/entities/";
      const id = decodeURIComponent(pathname.slice(pathname.indexOf(prefix) + prefix.length));
      store = store.filter((r) => r.id !== id);
      return { status: 204, body: null };
    }

    if (pathname.endsWith("/ngsi-ld/v1/temporal/entities") && method === "GET") {
      return { status: 200, body: [] };
    }

    if (pathname.endsWith("/schema/index.json") && method === "GET") {
      return { status: 200, body: { models: [{ version: 1 }] } };
    }

    if (pathname.endsWith("/schema/v1/json-schema") && method === "GET") {
      const defs = fixture?.schema ?? deriveSchema(store);
      return { status: 200, body: { $defs: defs } };
    }

    if (pathname.endsWith("/access") && method === "GET") {
      const access: AccessDocument = fixture?.access ?? {
        permissions: [{ resource: { type: "*" }, actions: ["*"], attributes: "*" }],
        prohibitions: [],
      };
      return { status: 200, body: access };
    }

    return { status: 400, body: { title: "Bad Request", status: 400 } };
  };

  const stub = transport as StubTransport;
  stub.calls = calls;
  stub.rows = () => store;
  return stub;
}

export function stubClient(fixture?: Fixture, config?: Partial<JcConfig>): Client & { transport: StubTransport } {
  const fullConfig: JcConfig = {
    slug: "demo",
    orgDomain: "example.org",
    space: "demo",
    transport: "bridge",
    ...config,
  };
  const transport = stubTransport(fixture);
  const client = createClient(fullConfig, transport);
  return Object.assign(client, { transport });
}

export function fakeContext(
  fixture?: Fixture & { user?: JcUser | null },
): { jc: DataClient; log(...parts: unknown[]): void; logs: unknown[][] } {
  const transport = stubTransport(fixture);
  const fullConfig: JcConfig = {
    slug: "demo",
    orgDomain: "example.org",
    space: "demo",
    transport: "bridge",
    user: fixture?.user ?? null,
  };
  const client = createClient(fullConfig, transport);
  const logs: unknown[][] = [];
  return {
    jc: client,
    log: (...parts: unknown[]) => logs.push(parts),
    logs,
  };
}
