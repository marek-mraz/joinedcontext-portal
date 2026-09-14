/**
 * What the assistant dock remembers between pages (UI-45).
 *
 * The run a person is talking to follows them from page to page, and a `navigate` event's
 * prefill has to survive the route change that delivers it. Both live in `sessionStorage`, so a
 * reload keeps them and a new tab starts clean; every read and write is guarded, because a
 * private window or a cleared site may refuse the storage and the dock must still render.
 */

const RUN_KEY = "jc.assistant.run";
const PREFILL_KEY = "jc.assistant.prefill";
const NOTICE_KEY = "jc.assistant.notice";
const CHANGED = "jc:assistant";
const OPEN_REQUEST = "jc:assistant-open";

export interface ActiveRun {
  project: string;
  runId: string;
}

/** The route rule of API/04 §4, mirrored here so a frame the server did not write is still refused. */
export function isPortalRoute(route: unknown): route is string {
  return (
    typeof route === "string" &&
    route.startsWith("/") &&
    !route.startsWith("//") &&
    route.length <= 512 &&
    !route.includes("#") &&
    !route.includes(":") &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u001f\u007f]/.test(route)
  );
}

function read(key: string): unknown {
  try {
    const raw = window.sessionStorage.getItem(key);
    return raw === null ? null : (JSON.parse(raw) as unknown);
  } catch {
    return null;
  }
}

function write(key: string, value: unknown): void {
  try {
    if (value === null) {
      window.sessionStorage.removeItem(key);
    } else {
      window.sessionStorage.setItem(key, JSON.stringify(value));
    }
  } catch {
    // Storage refused: the dock then lives for this page only.
  }
}

/** The stored run as the raw string: a stable snapshot for `useSyncExternalStore`. */
export function runSnapshot(): string | null {
  try {
    return window.sessionStorage.getItem(RUN_KEY);
  } catch {
    return null;
  }
}

export function parseRun(raw: string | null): ActiveRun | null {
  if (raw === null) {
    return null;
  }
  try {
    const value = JSON.parse(raw) as Partial<ActiveRun> | null;
    return value && typeof value.project === "string" && typeof value.runId === "string"
      ? { project: value.project, runId: value.runId }
      : null;
  } catch {
    return null;
  }
}

/** The run the dock follows from now on; `null` closes the dock. */
export function rememberRun(run: ActiveRun | null): void {
  write(RUN_KEY, run);
  window.dispatchEvent(new Event(CHANGED));
}

/** Called back whenever the run or the notice changes, in this tab. */
export function onAssistantChange(listener: () => void): () => void {
  window.addEventListener(CHANGED, listener);
  return () => {
    window.removeEventListener(CHANGED, listener);
  };
}

/**
 * The form values a `navigate` event carries for `route`, kept until the page takes them, and
 * the notice that the assistant navigated, kept until the person dismisses it: the dock is
 * remounted by the route change that delivers the event, so neither can live in its state.
 */
export function rememberPrefill(route: string, prefill: Record<string, unknown>): void {
  write(PREFILL_KEY, { route: route.split("?")[0], prefill });
  write(NOTICE_KEY, route);
  window.dispatchEvent(new Event(CHANGED));
}

/** The route the assistant last opened, until dismissed; a stable snapshot. */
export function noticeSnapshot(): string | null {
  const value = read(NOTICE_KEY);
  return typeof value === "string" ? value : null;
}

export function dismissNotice(): void {
  write(NOTICE_KEY, null);
  window.dispatchEvent(new Event(CHANGED));
}

/** The prefill left for this path, taken once: a second visit starts with an empty form. */
export function takePrefill(pathname: string): Record<string, unknown> | null {
  const value = read(PREFILL_KEY) as { route?: unknown; prefill?: unknown } | null;
  if (!value || value.route !== pathname) {
    return null;
  }
  write(PREFILL_KEY, null);
  return typeof value.prefill === "object" && value.prefill !== null
    ? (value.prefill as Record<string, unknown>)
    : null;
}

/** What the panel opens on: the chat, or the app builder (`build`). */
export type OpenIntent = "chat" | "build";

export function requestOpen(intent: OpenIntent = "chat"): void {
  window.dispatchEvent(new CustomEvent<OpenIntent>(OPEN_REQUEST, { detail: intent }));
}

export function onOpenRequest(listener: (intent: OpenIntent) => void): () => void {
  const handler = (event: Event) => {
    listener((event as CustomEvent<OpenIntent>).detail === "build" ? "build" : "chat");
  };
  window.addEventListener(OPEN_REQUEST, handler);
  return () => {
    window.removeEventListener(OPEN_REQUEST, handler);
  };
}
