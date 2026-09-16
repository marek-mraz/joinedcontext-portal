import { readCsrfToken } from "./client";

export type FindingLevel = "error" | "warning" | "info";

export interface Finding {
  level: FindingLevel;
  path: string;
  message: string;
}

export interface Verdict {
  ok: boolean;
  findings: Finding[];
  trace?: unknown;
  checkedAt: string;
  inputDigest: string;
}

export interface Draft {
  project: string;
  kind: string;
  name: string;
  manifest: Record<string, unknown>;
  verdict?: Verdict | null;
  touchedBy: string;
  touchedKind: "person" | "assistant" | "mcp" | "api-key" | "agent" | string;
  version: number;
  updatedAt: string;
}

export interface DraftEvent {
  project: string;
  kind: string;
  name: string;
  version: number;
  touchedBy: string;
  touchedKind: string;
  event: "put" | "verdict" | "drop";
  updatedAt: string;
}

/** Recursively sorts all object keys to ensure canonical serialization. */
export function canonicalizeJson(val: unknown): unknown {
  if (val === null || typeof val !== "object") {
    return val;
  }
  if (Array.isArray(val)) {
    return val.map(canonicalizeJson);
  }
  const sorted: Record<string, unknown> = {};
  for (const key of Object.keys(val as Record<string, unknown>).sort()) {
    const v = (val as Record<string, unknown>)[key];
    if (v !== undefined) {
      sorted[key] = canonicalizeJson(v);
    }
  }
  return sorted;
}

/**
 * Computes 64-bit FNV-1a hex digest of canonical JSON without third-party dependencies.
 * Matches backend `digest_of`.
 */
export function digestOf(value: unknown): string {
  const canonical = canonicalizeJson(value);
  const jsonStr = JSON.stringify(canonical);
  let hash = 0xcbf29ce484222325n;
  const prime = 0x100000001b3n;
  const mask = 0xffffffffffffffffn;
  const encoder = new TextEncoder();
  const bytes = encoder.encode(jsonStr);
  for (let i = 0; i < bytes.length; i++) {
    hash ^= BigInt(bytes[i]);
    hash = (hash * prime) & mask;
  }
  return hash.toString(16).padStart(16, "0");
}

function draftUrl(project: string, kind: string, name: string): string {
  return new URL(
    `/api/v1/projects/${encodeURIComponent(project)}/drafts/${encodeURIComponent(kind)}/${encodeURIComponent(name)}`,
    window.location.origin,
  ).toString();
}

export async function getDraft(
  project: string,
  kind: string,
  name: string,
): Promise<Draft | null> {
  try {
    const res = await fetch(
      new Request(draftUrl(project, kind, name), { credentials: "same-origin" }),
    );
    return res.ok ? asDraft(await res.json()) : null;
  } catch {
    return null;
  }
}

/** A draft answer carries a version; anything else is not a draft (a mocked or proxied 200). */
function asDraft(body: unknown): Draft {
  if (!body || typeof body !== "object" || typeof (body as Draft).version !== "number") {
    throw new Error("not a draft");
  }
  return body as Draft;
}

export async function putDraft(
  project: string,
  kind: string,
  name: string,
  manifest: unknown,
  expectedVersion?: number,
): Promise<Draft> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  const csrf = readCsrfToken();
  if (csrf) {
    headers["x-csrf-token"] = csrf;
  }
  const payload: { manifest: unknown; expectedVersion?: number } = { manifest };
  if (typeof expectedVersion === "number") {
    payload.expectedVersion = expectedVersion;
  }
  const res = await fetch(
    new Request(draftUrl(project, kind, name), {
      method: "PUT",
      credentials: "same-origin",
      headers,
      body: JSON.stringify(payload),
    }),
  );
  if (res.status === 409) {
    const errorData = (await res.json().catch(() => null)) as { current?: number } | null;
    throw Object.assign(new Error("draft conflict"), { status: 409, current: errorData?.current });
  }
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`failed to put draft (${res.status}): ${text}`);
  }
  return asDraft(await res.json());
}

const DRAFT_EVENTS = ["message", "draft", "put", "verdict", "drop"] as const;

export function subscribeDrafts(
  project: string,
  onEvent: (event: DraftEvent) => void,
): () => void {
  const Source = (globalThis as { EventSource?: typeof EventSource }).EventSource;
  if (!Source) {
    return () => {};
  }
  const source = new Source(
    `/api/v1/projects/${encodeURIComponent(project)}/drafts/events`,
    { withCredentials: true },
  );

  const handleMessage = (msg: MessageEvent<string>) => {
    try {
      onEvent(JSON.parse(msg.data) as DraftEvent);
    } catch {
      // ignore non-json messages
    }
  };

  for (const event of DRAFT_EVENTS) {
    source.addEventListener(event, handleMessage as EventListener);
  }

  return () => {
    for (const event of DRAFT_EVENTS) {
      source.removeEventListener(event, handleMessage as EventListener);
    }
    source.close();
  };
}
