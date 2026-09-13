import { useEffect } from "react";
import type { RefObject } from "react";
import { readCsrfToken } from "../../api/client";

/**
 * The write bridge of a sandboxed preview (AP-63). The frame has no origin and no session; a
 * form's save leaves it as a message, this page performs the write with the reviewer's own
 * session against the app's endpoint, and posts the answer back. Only the frame this page
 * created is heard, only the slug the preview was built for, only a PATCH of one entity's
 * attributes or a POST of one entity under that endpoint; anything else is dropped and counted.
 */

export interface KitWrite {
  kind: "kit-write";
  id: number;
  slug: string;
  method: "PATCH" | "POST";
  path: string;
  body: unknown;
}

const SLUG = /^[a-z2-7]{26,32}$/;
/** One path segment of an entity id, percent-encoded by the kit. */
const SEGMENT = "[A-Za-z0-9%._~-]+";

/** The message is a write this bridge forwards for `slug`. */
export function acceptsWrite(message: unknown, slug: string): message is KitWrite {
  if (typeof message !== "object" || message === null || !SLUG.test(slug)) {
    return false;
  }
  const m = message as Record<string, unknown>;
  if (m.kind !== "kit-write" || typeof m.id !== "number" || m.slug !== slug || typeof m.path !== "string") {
    return false;
  }
  if (typeof m.body !== "object" || m.body === null) {
    return false;
  }
  const base = `/api/endpoint/${slug}/ngsi-ld/v1/entities`;
  if (m.method === "POST") {
    return m.path === base;
  }
  if (m.method === "PATCH") {
    return new RegExp(`^${base.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}/${SEGMENT}/attrs$`).test(m.path);
  }
  return false;
}

export type Outcome = "forwarded" | "refused" | "ignored";

/**
 * One message: ignored when it is not from `source`, refused when it is not a write this
 * bridge forwards, forwarded otherwise, with the answer posted back to the frame.
 */
export async function handleKitMessage(
  event: { source: unknown; data: unknown },
  slug: string | undefined,
  source: Window | null | undefined,
  fetchImpl: typeof fetch = (input, init) => globalThis.fetch(input, init),
): Promise<Outcome> {
  if (!slug || !source || event.source !== source) {
    return "ignored";
  }
  if (!acceptsWrite(event.data, slug)) {
    return "refused";
  }
  const write = event.data;
  let status = 0;
  let body: unknown = null;
  try {
    const response = await fetchImpl(write.path, {
      method: write.method,
      credentials: "same-origin",
      headers: { "content-type": "application/json", "x-csrf-token": readCsrfToken() ?? "" },
      body: JSON.stringify(write.body),
    });
    status = response.status;
    body = await response.json().catch(() => null);
  } catch {
    status = 0;
  }
  source.postMessage({ kind: "kit-write-result", id: write.id, status, body }, "*");
  return "forwarded";
}

/** Listens for the frame's writes while the page is mounted. */
export function useKitWriteBridge(frame: RefObject<HTMLIFrameElement | null>, slug: string | undefined): void {
  useEffect(() => {
    if (!slug) {
      return undefined;
    }
    let refused = 0;
    const listener = (event: MessageEvent) => {
      void handleKitMessage(event, slug, frame.current?.contentWindow).then((outcome) => {
        if (outcome === "refused") {
          refused += 1;
          console.warn(`kit bridge: refused message ${refused} from the preview frame`);
        }
      });
    };
    window.addEventListener("message", listener);
    return () => window.removeEventListener("message", listener);
  }, [frame, slug]);
}
