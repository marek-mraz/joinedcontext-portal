import { useEffect } from "react";
import type { RefObject } from "react";
import { api, readCsrfToken } from "../../api/client";

/**
 * The bridge of a sandboxed preview (SDK-18, AP-63, Architecture/20 §5). The frame has no origin
 * and no session: every request of the SDK leaves it as a `jc-request`, this page performs it
 * with the reviewer's own session against the run's sandbox endpoint and posts a `jc-response`
 * back. Only the frame this page created is heard; reads under the endpoint pass always, writes
 * only for an operation the run's confirmed data needs name, `POST /functions/{fn}` to the run's
 * function route, which knows the run's files. Everything else is counted and never
 * forwarded; a refused request that carries an id is answered 403 at once, so the application
 * shows the refusal instead of waiting out its timeout. The frame's `jc-error` reports go to the
 * run for its repair.
 */

export interface PreviewError {
  message: string;
  file?: string;
  line?: number;
}

const SLUG = /^[a-z2-7]{26,32}$/;
/** One path segment; `plainSegment` says what it may decode to (an entity id is a URN). */
const SEGMENT = "[^/]+";
const ID = /^[A-Za-z0-9:._~-]+$/;
const READS = [
  new RegExp(`^ngsi-ld/v1/entities(/${SEGMENT})?$`),
  new RegExp(`^ngsi-ld/v1/temporal/entities(/${SEGMENT})?$`),
  /^schema\/index\.json$/,
  /^schema\/v\d+\/json-schema$/,
  /^access$/,
];
const WRITES: [string, RegExp, string][] = [
  ["POST", /^ngsi-ld\/v1\/entities$/, "createEntity"],
  ["PATCH", new RegExp(`^ngsi-ld/v1/entities/${SEGMENT}/attrs$`), "updateAttrs"],
  ["DELETE", new RegExp(`^ngsi-ld/v1/entities/${SEGMENT}$`), "deleteEntity"],
];
const FUNCTION = /^\/functions\/([a-z][a-z0-9-]{0,39})$/;
const BASE = "http://portal.invalid";
/** The server's own bounds on a reported error (API/04 §5), and how many one frame load relays. */
const MAX_MESSAGE = 2000;
const MAX_FILE = 256;
export const MAX_ERRORS = 20;

/**
 * A segment the gateway cannot read as anything but one segment: once decoded, no `/`, `\`, `%`
 * (a double encoding) or dot segment, because the gateway decodes before it normalises.
 */
function plainSegment(segment: string): boolean {
  let decoded: string;
  try {
    decoded = decodeURIComponent(segment);
  } catch {
    return false;
  }
  return ID.test(decoded) && !/^\.+$/.test(decoded);
}

/**
 * What a request is: `"read"`, the NGSI-LD operation of a write, or null when this bridge never
 * forwards it. The path must be exactly what the URL parser keeps, so the path checked is the
 * path fetched.
 */
export function operationOf(method: unknown, path: unknown, slug: string): string | null {
  if (typeof path !== "string" || !SLUG.test(slug)) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(path, BASE);
  } catch {
    return null;
  }
  const prefix = `/api/endpoint/${slug}/`;
  if (url.origin !== BASE || `${url.pathname}${url.search}` !== path || !url.pathname.startsWith(prefix)) {
    return null;
  }
  const rest = url.pathname.slice(prefix.length);
  if (!rest.split("/").every(plainSegment)) {
    return null;
  }
  if (method === "GET") {
    return READS.some((pattern) => pattern.test(rest)) ? "read" : null;
  }
  if (url.search !== "") {
    return null;
  }
  return WRITES.find(([verb, pattern]) => verb === method && pattern.test(rest))?.[2] ?? null;
}

/**
 * Where a `POST /functions/{fn}` of the frame goes: the run's function route under `route`, with
 * the request's query string; null for anything else.
 */
export function functionPathOf(method: unknown, path: unknown, route: string): string | null {
  if (method !== "POST" || typeof path !== "string") {
    return null;
  }
  let url: URL;
  try {
    url = new URL(path, BASE);
  } catch {
    return null;
  }
  const name = FUNCTION.exec(url.pathname)?.[1];
  if (url.origin !== BASE || `${url.pathname}${url.search}` !== path || !name) {
    return null;
  }
  return `${route}${name}${url.search}`;
}

/** The operations the run's confirmed data needs name (AP-22): the grant, not the prompt. */
export function grantedOperations(dataNeeds: unknown): Set<string> {
  const needs: unknown[] = Array.isArray(dataNeeds) ? dataNeeds : [];
  return new Set(
    needs.flatMap((need) => {
      const operations = (need as { operations?: unknown } | null)?.operations;
      return Array.isArray(operations) ? operations.filter((op): op is string => typeof op === "string") : [];
    }),
  );
}

/** A `jc-error` message as the run stores it, or null when it is not one. */
export function previewErrorOf(data: unknown): PreviewError | null {
  const m = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
  if (m.kind !== "jc-error" || typeof m.message !== "string" || m.message.trim() === "") {
    return null;
  }
  const error: PreviewError = { message: m.message.trim().slice(0, MAX_MESSAGE) };
  if (typeof m.file === "string" && m.file !== "") {
    error.file = m.file.slice(0, MAX_FILE);
  }
  if (typeof m.line === "number" && Number.isInteger(m.line) && m.line > 0) {
    error.line = m.line;
  }
  return error;
}

export interface Preview {
  slug: string | undefined;
  operations: ReadonlySet<string>;
  /** The run's function route, ending in `/functions/`. */
  functions?: string;
  /** The frame this page created. */
  source: Window | null | undefined;
}

export type Outcome = "forwarded" | "refused" | "relayed" | "ignored";

async function perform(
  method: string,
  path: string,
  body: unknown,
  fetchImpl: typeof fetch,
): Promise<{ status: number; body: unknown }> {
  const headers: Record<string, string> = { accept: "application/json" };
  if (method !== "GET") {
    headers["x-csrf-token"] = readCsrfToken() ?? "";
    if (body !== undefined) {
      headers["content-type"] = "application/json";
    }
  }
  try {
    const response = await fetchImpl(path, {
      method,
      credentials: "same-origin",
      headers,
      body: method !== "GET" && body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (response.status === 204) {
      return { status: 204, body: null };
    }
    const json = (response.headers.get("content-type") ?? "").includes("json");
    const text = await response.text().catch(() => "");
    let parsed: unknown = text === "" ? null : text;
    if (json && text !== "") {
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = text;
      }
    }
    return { status: response.status, body: parsed };
  } catch (err) {
    return { status: 0, body: { title: err instanceof Error ? err.message : String(err) } };
  }
}

/**
 * One message: ignored when it is not from the preview's frame, relayed when it is an error
 * report, forwarded when it is a request this bridge performs, refused otherwise.
 */
export async function handleBridgeMessage(
  event: { source: unknown; data: unknown },
  preview: Preview,
  report: (error: PreviewError) => void,
  fetchImpl: typeof fetch = (input, init) => globalThis.fetch(input, init),
): Promise<Outcome> {
  const { slug, source } = preview;
  if (!slug || !source || event.source !== source) {
    return "ignored";
  }
  const error = previewErrorOf(event.data);
  if (error) {
    report(error);
    return "relayed";
  }
  const m = (typeof event.data === "object" && event.data !== null ? event.data : {}) as Record<string, unknown>;
  if (m.kind !== "jc-request" || typeof m.id !== "number") {
    return "refused";
  }
  const fn = preview.functions ? functionPathOf(m.method, m.path, preview.functions) : null;
  if (fn !== null) {
    source.postMessage({ kind: "jc-response", id: m.id, ...(await perform("POST", fn, m.body, fetchImpl)) }, "*");
    return "forwarded";
  }
  const operation = operationOf(m.method, m.path, slug);
  const writesBody = m.method === "POST" || m.method === "PATCH";
  let refusal: string | null = null;
  if (operation === null) {
    refusal = "The preview may only read and write under its own endpoint and call its own functions.";
  } else if (operation !== "read" && !preview.operations.has(operation)) {
    refusal = `The run's data needs do not name ${operation}.`;
  } else if (writesBody && (typeof m.body !== "object" || m.body === null)) {
    refusal = "A write carries a JSON object.";
  }
  if (refusal !== null) {
    source.postMessage(
      { kind: "jc-response", id: m.id, status: 403, body: { title: "Refused by the preview bridge", status: 403, detail: refusal } },
      "*",
    );
    return "refused";
  }
  const answer = await perform(m.method as string, m.path as string, m.body, fetchImpl);
  source.postMessage({ kind: "jc-response", id: m.id, ...answer }, "*");
  return "forwarded";
}

/**
 * Serves the frame's requests while the page is mounted, and relays its first MAX_ERRORS
 * distinct error reports per preview document to the run.
 */
export function usePreviewBridge(
  frame: RefObject<HTMLIFrameElement | null>,
  run: { id: string; project: string; endpointSlug?: string; dataNeeds?: unknown; previewUrl?: string } | undefined,
): void {
  const id = run?.id;
  const project = run?.project;
  const slug = run?.endpointSlug;
  const previewUrl = run?.previewUrl;
  const needs = JSON.stringify(run?.dataNeeds ?? null);
  useEffect(() => {
    if (!id || !project || !slug) {
      return undefined;
    }
    const operations = grantedOperations(JSON.parse(needs));
    const functions = `/api/v1/projects/${encodeURIComponent(project)}/agent-runs/${encodeURIComponent(id)}/functions/`;
    const seen = new Set<string>();
    let refused = 0;
    const report = (error: PreviewError) => {
      const key = `${error.file ?? ""}:${error.line ?? ""}:${error.message}`;
      if (seen.has(key) || seen.size >= MAX_ERRORS) {
        return;
      }
      seen.add(key);
      void api
        .POST("/api/v1/projects/{project}/agent-runs/{id}/preview-errors", {
          params: { path: { project, id } },
          body: error,
        })
        .catch(() => undefined);
    };
    const listener = (event: MessageEvent) => {
      const preview = { slug, operations, functions, source: frame.current?.contentWindow };
      void handleBridgeMessage(event, preview, report).then((outcome) => {
        if (outcome === "refused") {
          refused += 1;
          console.warn(`preview bridge: refused message ${refused} from the preview frame`);
        }
      });
    };
    window.addEventListener("message", listener);
    return () => window.removeEventListener("message", listener);
  }, [frame, id, project, slug, needs, previewUrl]);
}
