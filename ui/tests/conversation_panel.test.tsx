/**
 * The conversation panel (T-0539, AP-53, UI-37).
 *
 * The workspace is the least trusted writer the Portal has, so the panel is tested against what
 * a compromised agent would send: markup in a thought, a question with no schema, a payload
 * missing the field the line is built from.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import { openQuestions } from "../src/pages/apps/useAgentRun";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

function panel(events: RunEvent[], streaming = true) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
        events={events}
        streaming={streaming}
        answering={false}
        onAnswer={() => {}}
      />
    </I18nextProvider>,
  );
}

describe("the conversation panel", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
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

  it("keeps the lines in stream order and labels the list", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "first" } },
      { seq: 2, kind: "thought", payload: { text: "second" } },
    ]);

    const list = screen.getByRole("list", { name: en.agentRun.conversation.title });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((row) => row.textContent)).toEqual(["thoughtfirst", "thoughtsecond"]);
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
