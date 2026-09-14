/**
 * The assistant drives the Portal (T-0609, UI-45, AG-45, AG-46).
 *
 * What is worth asserting is what a person cannot see from the chat: that a `navigate` frame
 * really moves the router and opens the form with the values, that the dock survives the move,
 * and that a frame naming anything but a path inside the Portal moves nothing.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
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

let requests: Request[] = [];
const fetchCalls = () => requests;

function renderPortal() {
  requests = [];
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    requests.push(request);
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
    if (path.endsWith("/agent-runs/new-conv-run-id")) {
      return json({
        ...RUN,
        id: "new-conv-run-id",
        appName: "",
        kind: "conversation",
        status: "interviewing",
        prompt: "Which datasets say anything about bikes?",
      });
    }
    if (path.endsWith("/assistant/conversations") && request.method === "POST") {
      return json(
        {
          ...RUN,
          id: "new-conv-run-id",
          appName: "",
          kind: "conversation",
          status: "interviewing",
        },
        202,
      );
    }
    if (path.endsWith("/agent-runs") && request.method === "GET") {
      return json({ items: [] });
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
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
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
    // And it is where it always is: the column on the right of the page, after the main
    // content, on this page as on the run page.
    const dock = notice.closest("aside") as HTMLElement;
    expect(dock.className).not.toContain("fixed");
    expect(dock.className).toContain("border-l");
    const main = document.querySelector("main") as HTMLElement;
    expect(main.compareDocumentPosition(dock) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("hides to a bubble at the bottom right, opens full screen, stops the run and closes", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;
    // The dock says which application the run builds, by name read as words.
    expect(within(dock).getByTestId("assistant-app")).toHaveTextContent("Ovzdusie dnes");
    expect(within(dock).getByTestId("assistant-app")).toHaveAttribute("title", RUN.appName);

    await user.click(within(dock).getByRole("button", { name: en.assistant.fullScreen }));
    expect(dock.className).toContain("fixed");
    await user.click(within(dock).getByRole("button", { name: en.assistant.sideView }));
    expect(dock.className).not.toContain("fixed");

    await user.click(within(dock).getByRole("button", { name: en.assistant.hide }));
    const bubble = screen.getByRole("button", { name: en.assistant.open });
    expect(bubble.className).toContain("fixed");
    expect(bubble.className).toContain("right-4");
    expect(bubble.className).toContain("bottom-4");
    await user.click(bubble);
    const reopened = screen.getByRole("button", { name: en.assistant.hide }).closest("aside") as HTMLElement;

    await user.click(within(reopened).getByRole("button", { name: en.assistant.cancel }));
    await waitFor(() => {
      expect(
        fetchCalls().some(
          (request) =>
            request.method === "POST" &&
            request.url.endsWith(`/agent-runs/${RUN_ID}/cancel`),
        ),
      ).toBe(true);
    });

    await user.click(within(reopened).getByRole("button", { name: en.assistant.close }));
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: en.assistant.hide })).not.toBeInTheDocument();
    });
    // Closing forgets the run; the bubble stays so the person can always ask again (UI-51).
    expect(screen.getByRole("button", { name: en.assistant.open })).toBeInTheDocument();
    expect(window.sessionStorage.getItem("jc.assistant.run")).toBeNull();
  });

  it("floats over the page, keeps that layout for the tab, and attaches a file from the composer", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;

    expect(within(dock).getByRole("button", { name: en.assistant.sideView })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await user.click(within(dock).getByRole("button", { name: en.assistant.floatView }));
    expect(dock.dataset.layout).toBe("float");
    expect(dock.className).toContain("fixed");
    expect(dock.className).toContain("shadow-xl");
    expect(within(dock).getByRole("button", { name: en.assistant.floatView })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(window.sessionStorage.getItem("jc.assistant.layout")).toBe("float");

    // The sample file is a paperclip in the composer, not a box of text under the chat.
    expect(within(dock).getByLabelText(en.assistant.attach)).toHaveAttribute("type", "file");
    expect(within(dock).queryByText(/Drop a CSV/)).not.toBeInTheDocument();
  });

  it("ignores a navigate frame that is not a path inside the Portal", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });
    const before = window.location.pathname;

    await emitToEveryStream("navigate", { seq: 8, route: "https://evil.example/", prefill: {} });
    await emitToEveryStream("navigate", { seq: 9, route: "//evil.example/x" });

    expect(window.location.pathname).toBe(before);
    expect(screen.queryByText(/opened/)).not.toBeInTheDocument();
  });

  it("renders bubble on page without a run, opens empty state and starts conversation on example click", async () => {
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/spaces`);
    renderPortal();

    const bubble = await screen.findByRole("button", { name: en.assistant.open });
    expect(bubble).toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(bubble);

    const emptyState = await screen.findByTestId("assistant-empty");
    expect(emptyState).toBeInTheDocument();
    expect(screen.getByText(i18n.t("assistant.empty.lead"))).toBeInTheDocument();

    const exampleButton = screen.getByRole("button", {
      name: i18n.t("assistant.empty.examples.find"),
    });
    await user.click(exampleButton);

    await waitFor(() => {
      expect(
        fetchCalls().some(
          (req) =>
            req.method === "POST" &&
            req.url.includes(`/projects/${PROJECT}/assistant/conversations`),
        ),
      ).toBe(true);
    });

    await waitFor(() => {
      expect(screen.queryByTestId("assistant-empty")).toBeNull();
    });
    expect(screen.getByRole("heading", { name: en.assistant.title })).toBeInTheDocument();
  });

  it("leaves full screen on Escape key", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;

    await user.click(within(dock).getByRole("button", { name: en.assistant.fullScreen }));
    expect(dock.className).toContain("fixed");

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(dock.className).not.toContain("fixed");
    });
  });

  it("shows busy dot on bubble while newest event is a person message", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });

    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;
    const user = userEvent.setup();
    await user.click(within(dock).getByRole("button", { name: en.assistant.hide }));

    const bubble = screen.getByRole("button", { name: en.assistant.open });
    expect(within(bubble).queryByTestId("assistant-busy")).toBeNull();

    await emitToEveryStream("message", {
      seq: 2,
      kind: "message",
      payload: { text: "Can you change the colors?", sentBy: "jana.kovacova" },
    });

    await waitFor(() => {
      expect(within(bubble).getByTestId("assistant-busy")).toBeInTheDocument();
    });

    await emitToEveryStream("thought", {
      seq: 3,
      kind: "thought",
      payload: { text: "Adjusting color scheme..." },
    });

    await waitFor(() => {
      expect(within(bubble).queryByTestId("assistant-busy")).toBeNull();
    });
  });
});
