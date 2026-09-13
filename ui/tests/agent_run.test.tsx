/**
 * One builder run, watched (T-0539, AG-43…AG-46, UI-34…UI-40).
 *
 * The things worth asserting are the ones a person cannot check by looking: that a reconnect's
 * replay does not double every line, that publishing is offered only when there is something to
 * publish, and that the preview frame is not handed the reviewer's own session.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AgentRunPage } from "../src/pages/apps/AgentRunPage";

const PROJECT = "banskabystrica";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";

const RUN = {
  id: RUN_ID,
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 12,
  tokensUsed: 48_210,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

/**
 * The browser's `EventSource`, as much of it as the hook uses. A stub rather than a mock server
 * because the interesting cases are the frames themselves: their order, their duplicates, and
 * the ones the Portal did not write.
 */
class StubEventSource {
  static opened: StubEventSource[] = [];
  readonly url: string;
  readonly withCredentials: boolean;
  closed = false;
  private readonly listeners = new Map<string, Set<EventListener>>();

  constructor(url: string, init?: EventSourceInit) {
    this.url = url;
    this.withCredentials = init?.withCredentials ?? false;
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

  close(): void {
    this.closed = true;
  }

  /** One named frame, exactly as the Portal writes it: `event:` the kind, `data:` the JSON. */
  emit(kind: string, payload: unknown, data?: string): void {
    const frame = new MessageEvent(kind, {
      data: data ?? JSON.stringify(payload),
      lastEventId: String((payload as { seq?: number } | undefined)?.seq ?? ""),
    });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

interface Options {
  run?: Record<string, unknown>;
  write?: { body: unknown; status: number };
}

function renderRun(options: Options = {}) {
  const { run = RUN, write = { body: { ok: true }, status: 200 } } = options;
  let current = run;

  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", roles: ["domain-editor"] });
    }
    if (request.method === "POST") {
      return json(write.body, write.status);
    }
    return json(current);
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <AgentRunPage project={PROJECT} runId={RUN_ID} onClose={() => {}} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return {
    fetchMock,
    view,
    /** What the next re-read of the record answers with, the way a status event makes it change. */
    setRun: (next: Record<string, unknown>) => {
      current = next;
    },
  };
}

const stream = () => StubEventSource.opened.at(-1) as StubEventSource;

async function emit(kind: string, payload: unknown, data?: string) {
  await act(async () => {
    stream().emit(kind, payload, data);
  });
}

describe("watching a run", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    StubEventSource.opened = [];
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("subscribes to the run's own stream with the session cookie", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    expect(stream().url).toBe(`/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/events`);
    // The stream is behind the same edge login as the Portal, so it needs the cookie.
    expect(stream().withCredentials).toBe(true);
  });

  it("shows the state the run is in, and the steps it has spent", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    const timeline = screen.getByRole("region", { name: en.agentRun.timeline.title });
    const current = within(timeline).getByText(en.agentRun.states.building);
    expect(current).toHaveAttribute("aria-current", "step");
    expect(
      within(timeline).getByText(
        en.agentRun.timeline.usage.replace("{steps}", "12").replace("{tokens}", "48,210"),
      ),
    ).toBeInTheDocument();
  });

  it("renders the agent's lines as they arrive", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    await emit("thought", { seq: 1, text: "Reading the projected schema" });
    await emit("tool", { seq: 2, tool: "bash", command: "cargo test", exitCode: 0 });

    expect(await screen.findByText("Reading the projected schema")).toBeInTheDocument();
    expect(
      screen.getByText(
        en.agentRun.line.tool
          .replace("{tool}", "bash")
          .replace("{command}", "cargo test")
          .replace("{exit}", "0"),
      ),
    ).toBeInTheDocument();
  });

  it("does not double a line the reconnect replayed (AG-45)", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    await emit("thought", { seq: 7, text: "Wiring the map" });
    // What a resume from `Last-Event-ID` brings back: the same frame, the same seq.
    await emit("thought", { seq: 7, text: "Wiring the map" });

    expect(await screen.findAllByText("Wiring the map")).toHaveLength(1);
  });

  it("drops a frame it cannot parse instead of rendering half of it", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    await emit("thought", undefined, "not json at all");

    expect(screen.getByText(en.agentRun.conversation.empty)).toBeInTheDocument();
  });

  it("re-reads the record when the run changes state rather than polling", async () => {
    const { setRun } = renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    setRun({ ...RUN, status: "testing" });
    await emit("status", { seq: 3, status: "testing" });

    const timeline = screen.getByRole("region", { name: en.agentRun.timeline.title });
    await waitFor(() => {
      expect(within(timeline).getByText(en.agentRun.states.testing)).toHaveAttribute(
        "aria-current",
        "step",
      );
    });
  });

  it("answers a question against the schema the agent asked for", async () => {
    const user = userEvent.setup();
    const { fetchMock } = renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    await emit("question", {
      seq: 4,
      questionId: "q-title",
      schema: {
        type: "object",
        required: ["title"],
        properties: { title: { type: "string", title: "Page title" } },
      },
    });

    await user.type(await screen.findByLabelText(/Page title/), "Air today");
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.answer }));

    await waitFor(async () => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.method === "POST");
      expect(request?.url).toContain(`/agent-runs/${RUN_ID}/answers`);
      const body = (await request!.json()) as Record<string, unknown>;
      expect(body.questionId).toBe("q-title");
      expect(body.answers).toEqual({ title: "Air today" });
    });
  });

  it("stops asking once the question is answered", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    await emit("question", {
      seq: 5,
      questionId: "q-title",
      schema: { type: "object", properties: { title: { type: "string" } } },
    });
    expect(await screen.findByText(en.agentRun.conversation.question)).toBeInTheDocument();

    await emit("answer", { seq: 6, questionId: "q-title" });

    await waitFor(() => {
      expect(screen.queryByText(en.agentRun.conversation.question)).not.toBeInTheDocument();
    });
  });

  it("offers publishing only once there is a preview to judge (AP-55)", async () => {
    const { setRun } = renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    const publish = screen.getByRole("button", { name: en.agentRun.publish });
    expect(publish).toBeDisabled();

    setRun({ ...RUN, status: "previewing", previewUrl: "/apps/ovzdusie-dnes-preview/" });
    await emit("status", { seq: 8, status: "previewing" });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeEnabled();
    });
  });

  it("frames the preview without handing it the reviewer's session (AP-19)", async () => {
    renderRun({ run: { ...RUN, status: "previewing", previewUrl: "/apps/ovzdusie-dnes-preview/" } });
    await screen.findByRole("heading", { name: RUN.appName });

    const frame = await screen.findByTitle(
      en.agentRun.preview.frameTitle.replace("{app}", RUN.appName),
    );
    expect(frame).toHaveAttribute("src", "/apps/ovzdusie-dnes-preview/");
    // `allow-scripts` with `allow-same-origin` on a same-origin frame is not a sandbox: the
    // frame could read the CSRF cookie and write as the person reviewing it.
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
  });

  it("watches the first minute on the left, then shows each pass in a fresh frame (UI-41, UI-42)", async () => {
    const { setRun } = renderRun({
      run: { ...RUN, status: "building", createdAt: new Date(Date.now() - 12_000).toISOString() },
    });
    await screen.findByRole("heading", { name: RUN.appName });

    // Before the first pass the preview column says which phase the run is in and how long it
    // has been running, rather than standing empty.
    const preview = screen.getByRole("region", { name: en.agentRun.preview.title });
    expect(within(preview).getByText(en.agentRun.states.building)).toBeInTheDocument();
    expect(within(preview).getByText(en.agentRun.preview.waiting)).toBeInTheDocument();
    expect(within(preview).getByText(/^1[0-9] s since the run started$/)).toBeInTheDocument();
    expect(screen.queryByTitle(en.agentRun.preview.frameTitle.replace("{app}", RUN.appName))).toBeNull();

    const first = `/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/preview?v=1`;
    setRun({ ...RUN, status: "previewing", previewUrl: first });
    await emit("status", { seq: 5, status: "previewing" });
    const frame = await screen.findByTitle(en.agentRun.preview.frameTitle.replace("{app}", RUN.appName));
    expect(frame).toHaveAttribute("src", first);

    // A second pass is a second URL: the frame is replaced, never left on the old document.
    const second = `/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/preview?v=2`;
    setRun({ ...RUN, status: "previewing", previewUrl: second });
    await emit("status", { seq: 9, status: "previewing" });
    await waitFor(() => {
      expect(
        screen.getByTitle(en.agentRun.preview.frameTitle.replace("{app}", RUN.appName)),
      ).toHaveAttribute("src", second);
    });
    // The chat stands beside the preview with its composer open.
    expect(screen.getByLabelText(en.agentRun.conversation.placeholder)).toBeEnabled();
  });

  it("says the app lives behind the platform login and nowhere else (ADR-N-019)", async () => {
    renderRun();
    await screen.findByRole("heading", { name: RUN.appName });

    expect(screen.getByText(en.agentRun.loginNote)).toBeInTheDocument();
  });

  it("cannot be stopped twice: a run that is over offers nothing", async () => {
    renderRun({ run: { ...RUN, status: "cancelled" } });
    await screen.findByRole("heading", { name: RUN.appName });

    expect(screen.getByRole("button", { name: en.agentRun.cancel })).toBeDisabled();
    expect(screen.getByRole("button", { name: en.agentRun.publish })).toBeDisabled();
    expect(screen.getByText(en.agentRun.states.cancelled)).toBeInTheDocument();
  });

  it("names why a stop or a publish was refused", async () => {
    const user = userEvent.setup();
    renderRun({
      write: {
        status: 409,
        body: {
          type: "https://joinedcontext.com/errors/conflict",
          title: "Conflict",
          status: 409,
          detail: "the run is already cancelled",
        },
      },
    });
    await screen.findByRole("heading", { name: RUN.appName });

    await user.click(screen.getByRole("button", { name: en.agentRun.cancel }));

    expect(await screen.findByText("the run is already cancelled")).toBeInTheDocument();
  });

  it("closes the stream when the page goes away", async () => {
    const { view } = renderRun();
    await screen.findByRole("heading", { name: RUN.appName });
    const source = stream();

    view.unmount();

    expect(source.closed).toBe(true);
  });
});
