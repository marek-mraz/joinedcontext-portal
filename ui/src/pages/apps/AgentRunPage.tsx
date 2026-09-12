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
  const { run, events, streaming, answer, cancel, publish } = useAgentRun(project, runId);

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
      {[cancel.error, publish.error, answer.error].map((error, index) =>
        error ? (
          <p role="alert" key={index} className="text-danger">
            {error instanceof ApiError ? (error.problem?.detail ?? error.message) : t("app.error.generic")}
          </p>
        ) : null,
      )}

      <RunTimeline status={record.status} steps={record.steps} tokensUsed={record.tokensUsed} />

      <ConversationPanel
        events={events}
        streaming={streaming}
        answering={answer.isPending}
        onAnswer={(questionId, answers) => {
          answer.mutate({ questionId, answers });
        }}
      />

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
            className="h-96 w-full rounded border border-border bg-surface"
          />
        </section>
      )}

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
  );
}
