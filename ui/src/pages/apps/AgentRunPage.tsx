import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { ChangeNotice } from "../../components/ChangeNotice";
import { isChange } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ConversationPanel } from "./ConversationPanel";
import { RunTimeline } from "./RunTimeline";
import { TERMINAL_STATES, useAgentRun } from "./useAgentRun";

/**
 * One builder run, live (UI-34…UI-40).
 *
 * The three things a person does here are answer a question, stop the run, and publish what it
 * built. Publishing is a merge request like any other change, which is why the answer to it is
 * the same notice the rest of the Portal shows (AP-55).
 */
export function AgentRunPage({
  project,
  runId,
  onClose,
}: {
  project: string;
  runId: string;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { run, events, streaming, answer, send, cancel, publish } = useAgentRun(project, runId);

  if (run.isPending) {
    return <p role="status">{t("agentRun.loading")}</p>;
  }
  if (run.isError || !run.data) {
    return (
      <p role="alert" className="text-danger">
        {t("agentRun.notFound")}
      </p>
    );
  }

  const record = run.data;
  const over = TERMINAL_STATES.includes(record.status);
  const published = publish.data;
  const change: Change | null = published && isChange(published) ? (published as Change) : null;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h1 className="text-xl font-bold">{record.appName}</h1>
          <p className="mt-1 text-sm text-fg-muted">
            {t("agentRun.subtitle", {
              endpoint: record.endpointName,
              appClass: record.appClass,
            })}
          </p>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("agentRun.back")}
        </button>
      </div>

      <p role="note" className="rounded border border-border bg-surface-subtle p-3 text-sm">
        {t("agentRun.loginNote")}
      </p>

      {change && <ChangeNotice change={change} project={project} />}
      {record.error !== undefined && record.error !== "" && (
        <p role="alert" className="text-danger">
          {record.error}
        </p>
      )}
      {[cancel.error, publish.error, answer.error, send.error].map((error, index) =>
        error ? (
          <p role="alert" key={index} className="text-danger">
            {error instanceof ApiError ? (error.problem?.detail ?? error.message) : t("app.error.generic")}
          </p>
        ) : null,
      )}

      {/*
        The conversation is the page: a person builds an app by talking to the agent, and
        watches what it built beside the talking. The preview and the controls sit in the
        column next to it, so neither has to be scrolled past to reach the other.
      */}
      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,22rem)]">
        <ConversationPanel
          events={events}
          streaming={streaming}
          answering={answer.isPending}
          sending={send.isPending}
          live={!over}
          onAnswer={(questionId, answers) => {
            answer.mutate({ questionId, answers });
          }}
          onSend={(text) => {
            send.mutate(text);
          }}
        />

        <div className="space-y-4 lg:sticky lg:top-4">
          {record.previewUrl !== undefined && record.previewUrl !== "" && (
            <section aria-labelledby="run-preview" className="space-y-2 rounded border border-border p-4">
              <h2 id="run-preview" className="text-base font-semibold">
                {t("agentRun.preview.title")}
              </h2>
              <p className="text-sm text-fg-muted">{t("agentRun.preview.hint")}</p>
              {/*
                No `allow-same-origin`: the app is served from the Portal's own origin, and that
                pair beside `allow-scripts` is not a sandbox at all — the frame could read the
                deliberately readable CSRF cookie and write as the signed-in reviewer (AP-19).
              */}
              <iframe
                title={t("agentRun.preview.frameTitle", { app: record.appName })}
                src={record.previewUrl}
                sandbox="allow-scripts"
                className="h-80 w-full rounded border border-border bg-surface"
              />
              <a
                href={record.previewUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-block text-sm text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
              >
                {t("agentRun.preview.open")}
              </a>
            </section>
          )}

          <RunTimeline status={record.status} steps={record.steps} tokensUsed={record.tokensUsed} />

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={over || cancel.isPending}
              onClick={() => {
                cancel.mutate();
              }}
              className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
            >
              {t("agentRun.cancel")}
            </button>
            <button
              type="button"
              disabled={record.status !== "previewing" || publish.isPending}
              onClick={() => {
                publish.mutate();
              }}
              className="rounded bg-primary px-4 py-1.5 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
            >
              {t("agentRun.publish")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
