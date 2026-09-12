import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { openQuestions } from "./useAgentRun";
import type { RunEvent } from "./useAgentRun";

/**
 * One line of the agent's work, as text.
 *
 * Every value here comes from the workspace, which is the least trusted writer the Portal has:
 * a payload is rendered as React children and never as markup, so a `<script>` in a thought is
 * a `<script>` on the screen and nothing else (AP-53).
 */
function line(event: RunEvent, t: (key: string, options?: Record<string, unknown>) => string): string {
  const payload = event.payload;
  const text = (key: string): string => {
    const value = payload[key];
    return typeof value === "string" ? value : "";
  };
  switch (event.kind) {
    case "status":
      return t("agentRun.line.status", { status: text("status") });
    case "thought":
      return text("text");
    case "tool":
      return t("agentRun.line.tool", {
        tool: text("tool"),
        command: text("command"),
        exit: String(payload.exitCode ?? ""),
      });
    case "commit":
      return t("agentRun.line.commit", {
        sha: text("sha").slice(0, 8),
        message: text("message"),
      });
    case "preview":
      return t("agentRun.line.preview", { url: text("previewUrl") });
    case "usage":
      return t("agentRun.line.usage", { tokens: String(payload.tokensThisStep ?? "") });
    case "answer":
      return t("agentRun.line.answer", { question: text("questionId") });
    case "question":
      return t("agentRun.line.question", { question: text("questionId") });
    case "lag":
      return t("agentRun.line.lag", { missed: String(payload.missed ?? "") });
    default:
      return event.kind;
  }
}

/**
 * The conversation: what the agent did, and the questions it is waiting on.
 *
 * The questions are the agent's own JSON Schema rendered by the Portal's form stack, so the
 * answer a person gives is validated against the schema the agent asked for before it is
 * posted (AG-45, UI-37).
 */
export function ConversationPanel({
  events,
  streaming,
  answering,
  onAnswer,
}: {
  events: RunEvent[];
  streaming: boolean;
  answering: boolean;
  onAnswer: (questionId: string, answers: unknown) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const questions = openQuestions(events);

  return (
    <section aria-labelledby="run-conversation" className="space-y-3 rounded border border-border p-4">
      <div className="flex items-baseline justify-between">
        <h2 id="run-conversation" className="text-base font-semibold">
          {t("agentRun.conversation.title")}
        </h2>
        <span className="text-xs text-fg-muted" role="status">
          {streaming ? t("agentRun.conversation.live") : t("agentRun.conversation.offline")}
        </span>
      </div>

      {events.length === 0 && <p className="text-sm text-fg-muted">{t("agentRun.conversation.empty")}</p>}

      <ol className="space-y-1 text-sm" aria-label={t("agentRun.conversation.title")}>
        {events.map((event) => (
          <li key={event.seq} className="flex gap-2">
            <span className="w-24 shrink-0 font-mono text-xs uppercase text-fg-muted">
              {event.kind}
            </span>
            <span className="break-words">{line(event, t)}</span>
          </li>
        ))}
      </ol>

      {questions.map((question) => (
        <div key={question.questionId} className="rounded border border-border bg-surface-subtle p-3">
          <h3 className="text-sm font-semibold">{t("agentRun.conversation.question")}</h3>
          <SchemaForm
            schema={question.schema as JsonSchema}
            disabled={answering}
            submitLabel={t("agentRun.conversation.answer")}
            onSubmit={(data) => {
              onAnswer(question.questionId, data);
            }}
          />
        </div>
      ))}
    </section>
  );
}
