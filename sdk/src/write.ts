/**
 * A form's write, the one thing the kit sends besides a read (AP-61…AP-63). The inputs come
 * from the endpoint's schema when the Portal inlined one; the write is a PATCH of attributes or
 * a POST of a new entity through the app's endpoint and nothing else. In the sandboxed preview
 * the frame has no session, so the write leaves as a message to the page that framed it and
 * comes back as its answer; a published app on the platform origin sends it itself.
 */
import type { Cell, Column } from "./ngsi";

/** One attribute as the endpoint's schema describes it: a JSON Schema property, trimmed. */
export interface FieldSchema {
  type?: string;
  enum?: string[];
  minimum?: number;
  maximum?: number;
  pattern?: string;
  format?: string;
}

/** One entity type: its properties and which are required. */
export interface TypeSchema {
  properties?: Record<string, FieldSchema>;
  required?: string[];
}

export type Schema = Record<string, TypeSchema>;

export type Input = "number" | "text" | "select" | "date" | "checkbox" | "geo";

export interface Field {
  name: string;
  input: Input;
  options?: string[];
  min?: number;
  max?: number;
  pattern?: string;
  required: boolean;
}

/** The input one attribute gets: the schema decides when it names the attribute, the rows otherwise. */
export function fieldOf(name: string, schema: TypeSchema | undefined, kind: Column): Field {
  const property = schema?.properties?.[name];
  const required = schema?.required?.includes(name) ?? false;
  if (!property) {
    return { name, input: kind === "number" ? "number" : kind === "geo" ? "geo" : kind === "date" ? "date" : "text", required };
  }
  if (Array.isArray(property.enum) && property.enum.length > 0) {
    return { name, input: "select", options: property.enum.map(String), required };
  }
  if (property.type === "number" || property.type === "integer") {
    return { name, input: "number", min: property.minimum, max: property.maximum, required };
  }
  if (property.type === "boolean") {
    return { name, input: "checkbox", required };
  }
  if (property.type === "object" || kind === "geo") {
    return { name, input: "geo", required };
  }
  if (property.format === "date-time" || property.format === "date" || kind === "date") {
    return { name, input: "date", required };
  }
  return { name, input: "text", pattern: property.pattern, required };
}

/** The NGSI-LD fragment of a patch: every cell a Property. */
export function attrsOf(patch: Record<string, Cell>): Record<string, { type: "Property"; value: Cell }> {
  return Object.fromEntries(Object.entries(patch).map(([name, value]) => [name, { type: "Property" as const, value }]));
}

export interface WriteResult {
  ok: boolean;
  status: number;
  /** The RFC 7807 `detail` (or `title`) when the endpoint refused. */
  detail?: string;
}

export interface Write {
  /** The entity patched; absent for a new one. */
  id?: string;
  type: string;
  /** For a new entity, its id; the patch otherwise. */
  entity?: Record<string, unknown>;
  patch?: Record<string, Cell>;
}

/** The message a sandboxed frame sends to its host page; the host performs the write. */
export interface BridgeRequest {
  kind: "kit-write";
  id: number;
  slug: string;
  method: "PATCH" | "POST";
  path: string;
  body: unknown;
}

export interface BridgeResult {
  kind: "kit-write-result";
  id: number;
  status: number;
  body?: unknown;
}

export const CSRF_COOKIE = "jc_csrf";
export const CSRF_HEADER = "x-csrf-token";
/** How long a preview waits for its host page to answer a write. */
export const BRIDGE_TIMEOUT_MS = 15000;

let sequence = 0;

/** The request a write is: method, path under the endpoint, body. */
export function requestOf(slug: string, write: Write): { method: "PATCH" | "POST"; path: string; body: unknown } {
  const base = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities`;
  if (write.id) {
    return { method: "PATCH", path: `${base}/${encodeURIComponent(write.id)}/attrs`, body: attrsOf(write.patch ?? {}) };
  }
  return { method: "POST", path: base, body: { type: write.type, ...write.entity } };
}

function detailOf(body: unknown, status: number): string | undefined {
  const problem = (typeof body === "object" && body !== null ? body : {}) as { detail?: unknown; title?: unknown };
  if (typeof problem.detail === "string" && problem.detail !== "") return problem.detail;
  if (typeof problem.title === "string" && problem.title !== "") return problem.title;
  return status >= 400 ? `The endpoint answered ${status}.` : undefined;
}

function csrfToken(): string {
  const found = document.cookie.split("; ").find((c) => c.startsWith(`${CSRF_COOKIE}=`));
  return found ? decodeURIComponent(found.slice(CSRF_COOKIE.length + 1)) : "";
}

async function direct(request: ReturnType<typeof requestOf>): Promise<WriteResult> {
  const response = await fetch(request.path, {
    method: request.method,
    credentials: "same-origin",
    headers: { "content-type": "application/json", [CSRF_HEADER]: csrfToken() },
    body: JSON.stringify(request.body),
  });
  const body: unknown = await response.json().catch(() => null);
  return { ok: response.ok, status: response.status, detail: response.ok ? undefined : detailOf(body, response.status) };
}

function viaBridge(slug: string, request: ReturnType<typeof requestOf>): Promise<WriteResult> {
  const id = ++sequence;
  return new Promise((resolve) => {
    const done = (result: WriteResult) => {
      window.removeEventListener("message", onMessage);
      clearTimeout(timer);
      resolve(result);
    };
    const onMessage = (event: MessageEvent) => {
      const data = event.data as Partial<BridgeResult> | null;
      if (!data || data.kind !== "kit-write-result" || data.id !== id || typeof data.status !== "number") return;
      done({ ok: data.status >= 200 && data.status < 300, status: data.status, detail: data.status < 300 ? undefined : detailOf(data.body, data.status) });
    };
    const timer = setTimeout(() => done({ ok: false, status: 0, detail: "The Portal did not answer the write." }), BRIDGE_TIMEOUT_MS);
    window.addEventListener("message", onMessage);
    const message: BridgeRequest = { kind: "kit-write", id, slug, ...request };
    window.parent.postMessage(message, "*");
  });
}

/** One write through the endpoint: by the host page in a preview, by this document otherwise. */
export function writeEntity(slug: string, write: Write, bridge: boolean): Promise<WriteResult> {
  const request = requestOf(slug, write);
  return bridge ? viaBridge(slug, request) : direct(request).catch((err: unknown) => ({ ok: false, status: 0, detail: err instanceof Error ? err.message : String(err) }));
}
