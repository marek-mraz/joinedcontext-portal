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
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import { openQuestions } from "../src/pages/apps/useAgentRun";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const sent = vi.fn();

function panel(events: RunEvent[], streaming = true) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
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
    expect(rows).toHaveLength(3);
    // The unknown kind still shows as its own name rather than an empty row.
    expect(rows[2]).toHaveTextContent("somethingNew");
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
    // An activity line carries its kind and no speaker at all.
    expect(rows[1]).toHaveTextContent("tool");
    expect(rows[1]).not.toHaveTextContent(en.agentRun.conversation.agent);
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
