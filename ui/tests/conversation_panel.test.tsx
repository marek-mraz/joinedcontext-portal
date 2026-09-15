/**
 * The conversation panel (T-0539, AP-53, UI-37).
 *
 * The workspace is the least trusted writer the Portal has, so the panel is tested against what
 * a compromised agent would send: markup in a thought, a question with no schema, a payload
 * missing the field the line is built from.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ConversationPanel, answeredSearches, line } from "../src/pages/apps/ConversationPanel";
import { EVENT_KINDS, openQuestions } from "../src/pages/apps/useAgentRun";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const sent = vi.fn();

function panel(events: RunEvent[], streaming = true) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
        project="helsinki"
        events={events}
        streaming={streaming}
        answering={false}
        sending={false}
        live
        onAnswer={() => {}}
        onSend={sent}
      />
    </I18nextProvider>,
  );
}

describe("the conversation panel", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    sent.mockClear();
  });

  it("renders markup from the workspace as text, never as markup (AP-53)", () => {
    const { container } = panel([
      {
        seq: 1,
        kind: "thought",
        payload: { text: "<img src=x onerror=alert(1)><script>alert(2)</script>" },
      },
    ]);

    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(
      screen.getByText("<img src=x onerror=alert(1)><script>alert(2)</script>"),
    ).toBeInTheDocument();
  });

  it("does not break on a payload missing the field the line needs", () => {
    panel([
      { seq: 1, kind: "tool", payload: {} },
      { seq: 2, kind: "commit", payload: { sha: 42 } },
      { seq: 3, kind: "somethingNew", payload: { text: "from a newer Portal" } },
    ]);

    const rows = within(
      screen.getByRole("list", { name: en.agentRun.conversation.title }),
    ).getAllByRole("listitem");
    // A kind this Portal has no words for draws nothing: a kind name is machinery (T-0772).
    expect(rows).toHaveLength(2);
    expect(screen.queryByText(/somethingNew/)).not.toBeInTheDocument();
  });

  it("leaves the page the assistant opened to the dock's notice, never a line with the event's kind (T-0772)", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "Drafted the grant." } },
      {
        seq: 2,
        kind: "navigate",
        payload: { route: "/projects/helsinki/access?grant=jana-kovacova-steward-helsinki" },
      },
    ]);

    const rows = within(
      screen.getByRole("list", { name: en.agentRun.conversation.title }),
    ).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(screen.queryByText(/navigate/i)).not.toBeInTheDocument();
  });

  it("never puts the name of an event's kind on screen", () => {
    const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;
    for (const kind of EVENT_KINDS) {
      expect(line({ seq: 1, kind, payload: {} }, t), kind).not.toBe(kind);
    }
  });

  it("keeps the turns in stream order, each under whoever said it", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "first" } },
      { seq: 2, kind: "message", payload: { text: "second", sentBy: "jana.kovacova" } },
      { seq: 3, kind: "thought", payload: { text: "third" } },
    ]);

    const list = screen.getByRole("list", { name: en.agentRun.conversation.title });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((row) => row.textContent)).toEqual([
      `${en.agentRun.conversation.agent}first`,
      "jana.kovacovasecond",
      `${en.agentRun.conversation.agent}third`,
    ]);
  });

  it("draws the shell work as activity rather than as something the agent said", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "Reading the endpoint." } },
      { seq: 2, kind: "tool", payload: { tool: "bash", command: "curl …", exitCode: 0 } },
    ]);

    const rows = within(
      screen.getByRole("list", { name: en.agentRun.conversation.title }),
    ).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent(en.agentRun.conversation.agent);
    // A tool line is an inspectable step named after its tool, with no speaker at all (AG-56).
    expect(rows[1]).toHaveTextContent("bash");
    expect(rows[1]).not.toHaveTextContent(en.agentRun.conversation.agent);
  });

  it("draws the run's status changes as one progress line, never as lines of their own", () => {
    const { unmount } = panel([
      { seq: 1, kind: "status", payload: { status: "queued" } },
      { seq: 2, kind: "status", payload: { status: "starting" } },
      { seq: 3, kind: "status", payload: { status: "interviewing" } },
      { seq: 4, kind: "thought", payload: { text: "Which feed?" } },
    ]);
    expect(screen.queryByText(/^Now /)).not.toBeInTheDocument();
    expect(screen.getAllByTestId("run-progress")).toHaveLength(1);
    expect(screen.getByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.waiting);
    unmount();

    panel([
      { seq: 3, kind: "status", payload: { status: "interviewing" } },
      { seq: 4, kind: "message", payload: { text: "The HSL one" } },
    ]);
    expect(screen.getByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.working);
  });

  it("keeps tokens, commits and preview addresses behind Details", async () => {
    const user = userEvent.setup();
    panel([
      { seq: 1, kind: "thought", payload: { text: "Building the map." } },
      { seq: 2, kind: "usage", payload: { tokensThisStep: 57285 } },
      { seq: 3, kind: "commit", payload: { sha: "3e26835f00", message: "map" } },
      { seq: 4, kind: "preview", payload: { previewUrl: "/api/v1/projects/helsinki/apps/x/preview" } },
    ]);
    const usage = screen.getByText("57285 tokens this step.");
    expect(usage).not.toBeVisible();
    await user.click(screen.getByText("Details (3 lines)"));
    expect(usage).toBeVisible();
    expect(screen.getByText(/Preview at/)).toBeVisible();
  });

  it("names the assistant's own steps by what they did", () => {
    panel([
      { seq: 1, kind: "tool", payload: { tool: "query_endpoint", status: "ok", input: { endpoint: "helsinki-all" } } },
      { seq: 2, kind: "tool", payload: { tool: "change_resource", status: "ok", input: { kind: "Pipeline" } } },
    ]);
    expect(screen.getByText("Read helsinki-all")).toBeInTheDocument();
    expect(screen.getByText(en.agentRun.step.label.change_resource)).toBeInTheDocument();
  });

  it("shows what the catalog search found only when the answer was about it", () => {
    const search = (seq: number): RunEvent => ({ seq, kind: "tool", payload: { tool: "search_catalog", status: "ok", output: { items: [] } } });
    const integrate: RunEvent[] = [
      { seq: 1, kind: "message", payload: { text: "Integrate the HSL feed" } },
      search(2),
      { seq: 3, kind: "tool", payload: { tool: "space_complete", status: "ok" } },
      { seq: 4, kind: "thought", payload: { text: "Drafted the space." } },
    ];
    expect([...answeredSearches(integrate)]).toEqual([]);
    panel(integrate);
    expect(screen.queryByText(en.agentRun.catalog.none)).not.toBeInTheDocument();

    expect([...answeredSearches([search(2), { seq: 3, kind: "thought", payload: { text: "Yes." } }])]).toEqual([2]);
    // A search nothing has followed yet is still what the conversation is about.
    expect([...answeredSearches([search(5)])]).toEqual([5]);
  });

  it("says whether the stream is live, so a stalled run is not read as a quiet one", () => {
    const { unmount } = panel([], true);
    expect(screen.getByText(en.agentRun.conversation.live)).toBeInTheDocument();
    unmount();

    panel([], false);
    expect(screen.getByText(en.agentRun.conversation.offline)).toBeInTheDocument();
  });

  it("shows nothing-yet rather than an empty box before the first frame", () => {
    panel([]);
    expect(screen.getByText(en.agentRun.conversation.empty)).toBeInTheDocument();
  });

  it("sends an instruction to a live run and empties the box", async () => {
    const user = userEvent.setup();
    panel([]);

    await user.type(
      screen.getByLabelText(en.agentRun.conversation.placeholder),
      "Sort by free bikes",
    );
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.send }));

    expect(sent).toHaveBeenCalledWith("Sort by free bikes");
    expect(screen.getByLabelText(en.agentRun.conversation.placeholder)).toHaveValue("");
  });

  it("will not send whitespace", async () => {
    const user = userEvent.setup();
    panel([]);

    await user.type(screen.getByLabelText(en.agentRun.conversation.placeholder), "   ");

    expect(screen.getByRole("button", { name: en.agentRun.conversation.send })).toBeDisabled();
    expect(sent).not.toHaveBeenCalled();
  });

  it("shows what the person already said, as text and not as markup", () => {
    const { container } = panel([
      {
        seq: 1,
        kind: "message",
        payload: { text: "<b>use a map</b>", sentBy: "jana.kovacova" },
      },
    ]);

    expect(container.querySelector("b")).toBeNull();
    expect(screen.getByText("<b>use a map</b>")).toBeInTheDocument();
    expect(screen.getByText("jana.kovacova")).toBeInTheDocument();
  });

  it("sends on Enter, and keeps Shift+Enter for a second line", async () => {
    const user = userEvent.setup();
    panel([]);
    const box = screen.getByLabelText(en.agentRun.conversation.placeholder);

    await user.type(box, "a bar chart{Shift>}{Enter}{/Shift}per station");
    expect(sent).not.toHaveBeenCalled();
    expect(box).toHaveValue("a bar chart\nper station");

    await user.type(box, "{Enter}");
    expect(sent).toHaveBeenCalledWith("a bar chart\nper station");
  });

  it("offers no box once the run is over", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <ConversationPanel
        project="helsinki"
          events={[]}
          streaming={false}
          answering={false}
          sending={false}
          live={false}
          onAnswer={() => {}}
          onSend={sent}
        />
      </I18nextProvider>,
    );

    expect(
      screen.queryByLabelText(en.agentRun.conversation.placeholder),
    ).not.toBeInTheDocument();
    expect(screen.getByText(en.agentRun.conversation.closed)).toBeInTheDocument();
  });

  it("renders the agent's question through the Portal's own form stack", () => {
    panel([
      {
        seq: 1,
        kind: "question",
        payload: {
          questionId: "q-refresh",
          schema: {
            type: "object",
            properties: { seconds: { type: "integer", title: "Refresh every" } },
          },
        },
      },
    ]);

    expect(screen.getByText(en.agentRun.conversation.question)).toBeInTheDocument();
    expect(screen.getByLabelText(/Refresh every/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.conversation.answer })).toBeEnabled();
  });
});

describe("openQuestions", () => {
  const question = (seq: number, id: string): RunEvent => ({
    seq,
    kind: "question",
    payload: { questionId: id, schema: { type: "object" } },
  });

  it("keeps the unanswered ones, oldest first", () => {
    const events = [question(1, "a"), question(2, "b"), { seq: 3, kind: "answer", payload: { questionId: "a" } }];
    expect(openQuestions(events).map((q) => q.questionId)).toEqual(["b"]);
  });

  it("ignores a question with no schema to render", () => {
    expect(
      openQuestions([{ seq: 1, kind: "question", payload: { questionId: "a" } }]),
    ).toEqual([]);
  });
});
