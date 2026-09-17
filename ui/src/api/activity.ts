import type { components } from "./schema";

export type ActivityEvent = components["schemas"]["ActivityEvent"];

export type Severity = "info" | "warning" | "error";

/** Every kind the vocabulary holds, in the order a filter offers them (Architecture/09 §6). */
export const ACTIVITY_KINDS = [
  "config.planned",
  "config.applied",
  "config.drifted",
  "change.merged",
  "pipeline.throughput",
  "pipeline.error",
  "pipeline.restarted",
  "endpoint.traffic",
  "access.denied",
  "mcp.tool",
  "agent.answer",
  "federation.forward",
  "federation.error",
  "catalogue.published",
] as const;

export const ACTIVITY_SOURCES = [
  "reconciler",
  "pipeline",
  "gateway",
  "broker",
  "ckan",
  "portal",
] as const;

/** What a view asks for; the list and the tail take the same thing. */
export interface ActivityQuery {
  space?: string;
  kind?: string;
  source?: string;
  severity?: Severity;
  since?: string;
  object?: string;
  limit?: number;
  cursor?: string;
}

export function activitySearch(query: ActivityQuery): Record<string, string> {
  return Object.fromEntries(
    Object.entries(query)
      .filter(([, value]) => value !== undefined && value !== "")
      .map(([key, value]) => [key, String(value)]),
  );
}

/**
 * The live tail. The Portal names each event after its kind, so a view subscribes to the kinds
 * it draws; `message` catches the rest. Returns the unsubscribe.
 */
export function subscribeActivity(
  project: string,
  query: ActivityQuery,
  onEvent: (event: ActivityEvent) => void,
): () => void {
  const Source = (globalThis as { EventSource?: typeof EventSource }).EventSource;
  if (!Source) {
    return () => {};
  }
  const search = new URLSearchParams(activitySearch(query)).toString();
  const source = new Source(
    `/api/v1/projects/${encodeURIComponent(project)}/activity/stream${search ? `?${search}` : ""}`,
    { withCredentials: true },
  );
  const handle = (message: MessageEvent<string>) => {
    try {
      onEvent(JSON.parse(message.data) as ActivityEvent);
    } catch {
      // A keep-alive comment is not an event.
    }
  };
  for (const kind of [...ACTIVITY_KINDS, "message"]) {
    source.addEventListener(kind, handle as EventListener);
  }
  return () => {
    for (const kind of [...ACTIVITY_KINDS, "message"]) {
      source.removeEventListener(kind, handle as EventListener);
    }
    source.close();
  };
}

/**
 * The tail in front of the page it is tailing: newest first, no event twice, and never more
 * than the window a person can read. The identity of an event is its time and its summary —
 * the store hands out no id, and two events of the same kind a minute apart are two events.
 */
export const TAIL_WINDOW = 200;

export function mergeActivity(
  known: ActivityEvent[],
  arriving: ActivityEvent[],
): ActivityEvent[] {
  const seen = new Set(known.map(identity));
  const fresh = arriving.filter((event) => !seen.has(identity(event)));
  if (fresh.length === 0) {
    return known;
  }
  return [...fresh, ...known]
    .sort((a, b) => b.time.localeCompare(a.time))
    .slice(0, TAIL_WINDOW);
}

function identity(event: ActivityEvent): string {
  return `${event.time}|${event.kind}|${event.summary}|${event.space ?? ""}`;
}

/** The object page an event opens, as `{plural}/{name}`. */
export function objectOf(event: ActivityEvent): string | undefined {
  const details = event.details as Record<string, unknown> | undefined;
  const object = details?.object;
  return typeof object === "string" && object.includes("/") ? object : undefined;
}
