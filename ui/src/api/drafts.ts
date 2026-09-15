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

/** A `Request` needs an absolute URL outside a document (jsdom included), as client.ts notes. */
function absolute(path: string): string {
  return new URL(path, window.location.origin).toString();
}

export async function getDraft(
  project: string,
  kind: string,
  name: string,
): Promise<Draft | null> {
  try {
    const res = await fetch(
      new Request(
        absolute(
          `/api/v1/projects/${encodeURIComponent(project)}/drafts/${encodeURIComponent(kind)}/${encodeURIComponent(name)}`,
        ),
        { credentials: "same-origin" },
      ),
    );
    if (!res.ok) {
      return null;
    }
    return asDraft(await res.json());
  } catch {
    return null;
  }
}

/** A draft answer carries a version; anything else is not a draft (a mocked or proxied 200). */
function asDraft(body: unknown): Draft {
  if (
    !body ||
    typeof body !== "object" ||
    typeof (body as Draft).version !== "number"
  ) {
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
  const headers: Record<string, string> = {
    "content-type": "application/json",
  };
  const csrf = readCsrfToken();
  if (csrf) {
    headers["x-csrf-token"] = csrf;
  }
  const payload: { manifest: unknown; expectedVersion?: number } = { manifest };
  if (typeof expectedVersion === "number") {
    payload.expectedVersion = expectedVersion;
  }
  const res = await fetch(
    new Request(
      absolute(
        `/api/v1/projects/${encodeURIComponent(project)}/drafts/${encodeURIComponent(kind)}/${encodeURIComponent(name)}`,
      ),
      {
        method: "PUT",
        credentials: "same-origin",
        headers,
        body: JSON.stringify(payload),
      },
    ),
  );
  if (res.status === 409) {
    const errorData = (await res.json().catch(() => null)) as {
      current?: number;
    } | null;
    const err = new Error("draft conflict");
    (err as unknown as { status: number; current?: number }).status = 409;
    (err as unknown as { status: number; current?: number }).current =
      errorData?.current;
    throw err;
  }
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`failed to put draft (${res.status}): ${text}`);
  }
  return asDraft(await res.json());
}

export function subscribeDrafts(
  project: string,
  onEvent: (event: DraftEvent) => void,
): () => void {
  const Source = (globalThis as { EventSource?: typeof EventSource })
    .EventSource;
  if (!Source) {
    return () => {};
  }
  const source = new Source(
    `/api/v1/projects/${encodeURIComponent(project)}/drafts/events`,
    { withCredentials: true },
  );

  const handleMessage = (msg: MessageEvent<string>) => {
    try {
      const data = JSON.parse(msg.data) as DraftEvent;
      onEvent(data);
    } catch {
      // ignore non-json messages
    }
  };

  source.addEventListener("message", handleMessage as EventListener);
  source.addEventListener("draft", handleMessage as EventListener);
  source.addEventListener("put", handleMessage as EventListener);
  source.addEventListener("verdict", handleMessage as EventListener);
  source.addEventListener("drop", handleMessage as EventListener);

  source.onerror = () => {
    // browser handles reconnects
  };

  return () => {
    source.removeEventListener("message", handleMessage as EventListener);
    source.removeEventListener("draft", handleMessage as EventListener);
    source.removeEventListener("put", handleMessage as EventListener);
    source.removeEventListener("verdict", handleMessage as EventListener);
    source.removeEventListener("drop", handleMessage as EventListener);
    source.close();
  };
}
