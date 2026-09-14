/**
 * The assistant drives the Portal (T-0609, UI-45, AG-45, AG-46).
 *
 * What is worth asserting is what a person cannot see from the chat: that a `navigate` frame
 * really moves the router and opens the form with the values, that the dock survives the move,
 * and that a frame naming anything but a path inside the Portal moves nothing.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { isPortalRoute, rememberPrefill, takePrefill } from "../src/assistant/state";

const PROJECT = "banskabystrica";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";

const RUN = {
  id: RUN_ID,
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "static",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 3,
  tokensUsed: 1_200,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["domain-editor"] };

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "ovzdusie", project: PROJECT },
      spec: { tenant: "ovzdusie" },
    },
  ],
};

class StubEventSource {
  static opened: StubEventSource[] = [];
  readonly url: string;
  private readonly listeners = new Map<string, Set<EventListener>>();

  constructor(url: string) {
    this.url = url;
    StubEventSource.opened.push(this);
  }

  addEventListener(kind: string, listener: EventListener): void {
    const set = this.listeners.get(kind) ?? new Set<EventListener>();
    set.add(listener);
    this.listeners.set(kind, set);
  }

  removeEventListener(kind: string, listener: EventListener): void {
    this.listeners.get(kind)?.delete(listener);
  }

  close(): void {}

  emit(kind: string, payload: Record<string, unknown>): void {
    const frame = new MessageEvent(kind, {
      data: JSON.stringify(payload),
      lastEventId: String(payload.seq ?? ""),
    });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

function renderPortal() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url, "http://localhost").pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith(`/agent-runs/${RUN_ID}`)) {
      return json(RUN);
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The dock's stream: every dock opens one, and the run page's own comes first. */
const dockStream = () => StubEventSource.opened.at(-1) as StubEventSource;

async function emitToEveryStream(kind: string, payload: Record<string, unknown>) {
  await act(async () => {
    for (const stream of StubEventSource.opened) {
      stream.emit(kind, payload);
    }
  });
}

describe("the route rule", () => {
  it("lets only a path inside the Portal through", () => {
    expect(isPortalRoute("/projects/helsinki/endpoints?tab=all")).toBe(true);
    expect(isPortalRoute("/")).toBe(true);
    for (const bad of [
      "https://evil.example/",
      "//evil.example/projects",
      "javascript:alert(1)",
      "projects/helsinki",
      "/projects/helsinki#x",
      "/projects/hel\nsinki",
      `/${"a".repeat(512)}`,
      "",
      42,
      null,
    ]) {
      expect(isPortalRoute(bad), String(bad)).toBe(false);
    }
  });

  it("hands a prefill to its own path exactly once", () => {
    rememberPrefill("/projects/x/endpoints?tab=all", { name: "air" });
    expect(takePrefill("/projects/x/pipelines")).toBeNull();
    expect(takePrefill("/projects/x/endpoints")).toEqual({ name: "air" });
    expect(takePrefill("/projects/x/endpoints")).toBeNull();
  });
});

describe("the assistant dock", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    StubEventSource.opened = [];
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/apps?run=${RUN_ID}`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("follows a navigate frame to the endpoints page and opens the form prefilled", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: RUN.appName });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });

    await emitToEveryStream("navigate", {
      seq: 7,
      route: `/projects/${PROJECT}/endpoints`,
      prefill: { name: "air-quality-public", contextSpaceRef: "ovzdusie" },
    });

    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/endpoints`);
    });
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => {
      expect((dialog.querySelector('input[name="name"], #root_name') as HTMLInputElement).value).toBe(
        "air-quality-public",
      );
    });
    // The dock came along, and says what it did.
    const notice = await screen.findByText(`The assistant opened /projects/${PROJECT}/endpoints`);
    expect(notice).toBeInTheDocument();
    expect(dockStream().url).toBe(`/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/events`);
    // And it is where it always is: the bottom-left corner, on this page as on the run page.
    const dock = notice.closest(".fixed") as HTMLElement;
    expect(dock.className).toContain("left-4");
    expect(dock.className).not.toContain("right-4");
  });

  it("ignores a navigate frame that is not a path inside the Portal", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: RUN.appName });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });
    const before = window.location.pathname;

    await emitToEveryStream("navigate", { seq: 8, route: "https://evil.example/", prefill: {} });
    await emitToEveryStream("navigate", { seq: 9, route: "//evil.example/x" });

    expect(window.location.pathname).toBe(before);
    expect(screen.queryByText(/opened/)).not.toBeInTheDocument();
  });
});
