import { useEffect, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { openQuestions } from "./useAgentRun";
import { ActionStep } from "./ActionStep";
import { CatalogCards, catalogItemsOf } from "./CatalogCards";
import { EndpointProposalCard, proposalOf } from "./EndpointProposalCard";
import { KpiCard, kpiOf } from "./KpiCard";
import { KpiPipelineCard, kpiPipelineOf } from "./KpiPipelineCard";
import { QueryResultCard, queryResultOf } from "./QueryResultCard";
import type { RunEvent } from "./useAgentRun";

/** Who a line came from. The three read differently, so they are drawn differently. */
export type Speaker = "agent" | "person" | "activity";

export function speakerOf(kind: string): Speaker {
  if (kind === "thought" || kind === "question") {
    return "agent";
  }
  if (kind === "message" || kind === "answer") {
    return "person";
  }
  return "activity";
}

/** What makes two tool steps the same step: the tool, how it ended and why. */
function stepKey(event: RunEvent): string {
  const { tool, status, error, exitCode } = event.payload;
  return JSON.stringify([tool, status, error ?? null, exitCode ?? null]);
}

/**
 * The events as the transcript draws them: a tool step repeated in a row with the same outcome
 * is one step with a count, the newest occurrence standing for all of them, so a function the
 * preview calls on every render does not bury the conversation. A repeat after anything else
 * starts a new step.
 */
export function foldRepeats(events: RunEvent[]): { event: RunEvent; count: number }[] {
  const folded: { event: RunEvent; count: number }[] = [];
  for (const event of events) {
    const last = folded.at(-1);
    if (
      last !== undefined &&
      event.kind === "tool" &&
      last.event.kind === "tool" &&
      stepKey(event) === stepKey(last.event)
    ) {
      folded[folded.length - 1] = { event, count: last.count + 1 };
    } else {
      folded.push({ event, count: 1 });
    }
  }
  return folded;
}

/**
 * One line of the run, as text.
 *
 * Every value here comes from the workspace, which is the least trusted writer the Portal has:
 * a payload is rendered as React children and never as markup, so a `<script>` in a thought is
 * a `<script>` on the screen and nothing else (AP-53).
 */
export function line(
  event: RunEvent,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
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
    case "message":
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
    case "endpoints": {
      const names = Array.isArray(payload.names)
        ? payload.names.filter((name): name is string => typeof name === "string")
        : [];
      return names.length > 0
        ? t("agentRun.line.endpoints", { names: names.join(", ") })
        : t("agentRun.line.noEndpoints");
    }
    default:
      return event.kind;
  }
}

/** Whose name goes above a bubble: the person who typed it, or the agent. */
function labelOf(
  event: RunEvent,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const who = event.payload.sentBy;
  if (typeof who === "string" && who !== "") {
    return who;
  }
  return speakerOf(event.kind) === "person"
    ? t("agentRun.conversation.you")
    : t("agentRun.conversation.agent");
}

/**
 * The conversation with the agent: what it said, what it did, the questions it is waiting on,
 * and the box a person writes the next instruction in.
 *
 * A run is a conversation rather than a form, which is why the composer is open the whole time
 * and not only while a question stands: somebody watching a build go the wrong way says so
 * instead of cancelling and starting again. What the agent said and what a person typed are
 * drawn as turns; the shell commands and status changes between them stay as thin activity
 * lines, so the conversation is readable at a glance and the work is still all there. The
 * questions are the agent's own JSON Schema rendered by the Portal's form stack, so an answer
 * is validated against the schema that asked for it before it is posted (AG-45, UI-37).
 */
export function ConversationPanel({
  project,
  events,
  streaming,
  answering,
  sending,
  live,
  onAnswer,
  onSend,
  attach,
  above,
  onUseEndpoint,
  usedEndpoints,
}: {
  /** The project the run belongs to: what a card's links open. */
  project: string;
  events: RunEvent[];
  streaming: boolean;
  answering: boolean;
  sending: boolean;
  /** Whether the run still reads: a finished one takes no more instructions. */
  live: boolean;
  onAnswer: (questionId: string, answers: unknown) => void;
  onSend: (text: string) => void;
  /** A control beside the text box, such as the dock's attach button. */
  attach?: ReactNode;
  /** A row above the text box, such as the dock's data bar. */
  above?: ReactNode;
  /** Adds an endpoint the catalog search found to the conversation's data (AG-75). */
  onUseEndpoint?: (name: string) => void;
  /** The endpoints the conversation queries, so a found one says it is in use. */
  usedEndpoints?: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  const questions = openQuestions(events);
  const foot = useRef<HTMLDivElement>(null);

  // A chat that does not follow its own newest line is a log. `block: "nearest"` keeps the
  // scrolling inside the transcript rather than dragging the whole page; jsdom has no such
  // method at all, which is why it is called only where it exists.
  useEffect(() => {
    foot.current?.scrollIntoView?.({ block: "nearest" });
  }, [events.length]);

  const send = (): void => {
    const text = draft.trim();
    if (text === "" || sending) {
      return;
    }
    onSend(text);
    setDraft("");
  };

  return (
    <section
      aria-labelledby="run-conversation"
      className="flex min-h-[28rem] flex-col rounded border border-border"
    >
      <div className="flex items-baseline justify-between border-b border-border px-4 py-2">
        <h2 id="run-conversation" className="text-base font-semibold">
          {t("agentRun.conversation.title")}
        </h2>
        <span className="text-xs text-fg-muted" role="status">
          {streaming ? t("agentRun.conversation.live") : t("agentRun.conversation.offline")}
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {events.length === 0 && (
          <p className="text-sm text-fg-muted">{t("agentRun.conversation.empty")}</p>
        )}

        <ol className="space-y-2 text-sm" aria-label={t("agentRun.conversation.title")}>
          {foldRepeats(events).map(({ event, count }) => {
            const speaker = speakerOf(event.kind);
            if (event.kind === "tool") {
              // What the assistant found is drawn as cards above the step itself (UI-46).
              const found =
                event.payload.tool === "search_catalog" ? catalogItemsOf(event.payload.output) : null;
              const proposal =
                event.payload.tool === "propose_endpoint" ? proposalOf(event.payload.output) : null;
              const kpi =
                event.payload.tool === "compute_kpi" ? kpiOf(event.payload.output, event.payload.input) : null;
              const queried = queryResultOf(event.payload);
              const kpiPipeline =
                event.payload.tool === "draft_kpi_pipeline" ? kpiPipelineOf(event.payload.output) : null;
              return (
                <li key={event.seq} className="space-y-2">
                  {found !== null ? (
                    <CatalogCards
                      project={project}
                      items={found}
                      onUseEndpoint={live ? onUseEndpoint : undefined}
                      usedEndpoints={usedEndpoints}
                    />
                  ) : null}
                  {proposal !== null ? (
                    <EndpointProposalCard project={project} proposal={proposal} />
                  ) : null}
                  {kpi !== null ? <KpiCard project={project} kpi={kpi} onSend={live ? onSend : undefined} /> : null}
                  {kpiPipeline !== null ? <KpiPipelineCard project={project} pipeline={kpiPipeline} /> : null}
                  {queried !== null ? <QueryResultCard result={queried} /> : null}
                  <ActionStep event={event} live={live} onSend={onSend} count={count} />
                </li>
              );
            }
            if (event.kind === "endpoints") {
              return (
                <li key={event.seq} data-testid="endpoints-line" className="flex items-center gap-1.5 text-xs text-fg-muted">
                  <span aria-hidden className="h-px flex-1 bg-border" />
                  <span className="min-w-0 break-words">{line(event, t)}</span>
                  <span aria-hidden className="h-px flex-1 bg-border" />
                </li>
              );
            }
            if (speaker === "activity") {
              return (
                <li key={event.seq} className="flex gap-2 text-xs text-fg-muted">
                  <span className="w-16 shrink-0 font-mono uppercase">{event.kind}</span>
                  <span className="min-w-0 break-words font-mono">{line(event, t)}</span>
                </li>
              );
            }
            const mine = speaker === "person";
            return (
              <li key={event.seq} className={mine ? "flex justify-end" : "flex"}>
                <div className="max-w-[85%]">
                  <p className={mine ? "text-right text-xs text-fg-muted" : "text-xs text-fg-muted"}>
                    {labelOf(event, t)}
                  </p>
                  <p
                    className={
                      mine
                        ? "mt-0.5 whitespace-pre-wrap break-words rounded-lg rounded-br-sm bg-primary px-3 py-2 text-primary-fg"
                        : "mt-0.5 whitespace-pre-wrap break-words rounded-lg rounded-bl-sm bg-surface-subtle px-3 py-2"
                    }
                  >
                    {line(event, t)}
                  </p>
                </div>
              </li>
            );
          })}
        </ol>

        {questions.map((question) => (
          <div
            key={question.questionId}
            className="mt-3 rounded-lg border border-border bg-surface-subtle p-3"
          >
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

        <div ref={foot} />
      </div>

      {live ? (
        <form
          className="flex flex-col gap-2 border-t border-border p-3"
          onSubmit={(event) => {
            event.preventDefault();
            send();
          }}
        >
          {above}
          <div className="flex items-end gap-2">
            {attach}
            <label className="sr-only" htmlFor="run-message">
              {t("agentRun.conversation.placeholder")}
            </label>
            <textarea
              id="run-message"
              rows={2}
              value={draft}
              placeholder={t("agentRun.conversation.placeholder")}
              onChange={(event) => {
                setDraft(event.target.value);
              }}
              // Enter sends, as every chat box does; a newline is still Shift+Enter.
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  send();
                }
              }}
              className="block min-w-0 flex-1 resize-none rounded border border-border bg-surface px-3 py-1.5 text-sm"
            />
            <button
              type="submit"
              disabled={draft.trim() === "" || sending}
              className="rounded bg-primary px-3 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
            >
              {t("agentRun.conversation.send")}
            </button>
          </div>
        </form>
      ) : (
        <p className="border-t border-border p-3 text-sm text-fg-muted">
          {t("agentRun.conversation.closed")}
        </p>
      )}
    </section>
  );
}
