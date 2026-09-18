import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { ChangeNotice } from "../../components/ChangeNotice";
import { isChange } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { usePreviewBridge } from "./previewBridge";
import { RunPublication } from "./RunPublication";
import { RunTimeline } from "./RunTimeline";
import { TERMINAL_STATES, useAgentRun } from "./useAgentRun";
import type { RunEvent } from "./useAgentRun";
import { rememberRun } from "../../assistant/state";
import { appDisplayName, useEndpointTitles } from "./appTitle";
import { Button, PageHeader } from "../../components/ui";

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
  const { run, events, cancel, publish } = useAgentRun(project, runId);
  const endpointTitles = useEndpointTitles(project);
  // The conversation lives in the shell's assistant dock, which follows the person to the
  // pages the assistant opens (UI-45); this page only tells it which run to show.
  useEffect(() => {
    rememberRun({ project, runId });
  }, [project, runId]);
  // The preview's reads and writes reach the endpoint through this page, never from the frame (AP-63).
  const frame = useRef<HTMLIFrameElement>(null);
  usePreviewBridge(frame, run.data, run.data !== undefined && !TERMINAL_STATES.includes(run.data.status));

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
  const endpointTitle = endpointTitles.get(record.endpointName);
  const displayName = appDisplayName({ title: record.title, appName: record.appName, endpointTitle });
  const over = TERMINAL_STATES.includes(record.status);
  // A run with a preview publishes; an unattended one ends waiting for approval with its preview
  // built and publishes from there, once (AG-69, AP-71).
  const publishable =
    record.status === "previewing" ||
    (record.unattended === true && record.status === "awaiting_approval" && !record.changeId);
  const published = publish.data;
  const change: Change | null = published && isChange(published) ? (published as Change) : null;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <PageHeader
          title={displayName}
          description={
            <>
              {t("agentRun.subtitle", {
                endpoint: endpointTitle ?? record.endpointName,
                appClass: record.appClass,
              })}{" "}
              <span className="font-mono text-caption">{record.appName}</span>
            </>
          }
        />
        <Button onClick={onClose}>
          {t("agentRun.back")}
        </Button>
      </div>

      {change && !record.changeId && <ChangeNotice change={change} project={project} />}
      <RunPublication
        project={project}
        sourceUrl={record.sourceUrl}
        changeId={record.changeId}
        onApproved={() => void run.refetch()}
      />
      {record.error !== undefined && record.error !== "" && (
        <p role="alert" className="text-danger">
          {record.error}
        </p>
      )}
      {[cancel.error, publish.error].map((error, index) =>
        error ? (
          <p role="alert" key={index} className="text-danger">
            {error instanceof ApiError ? (error.problem?.detail ?? error.message) : t("app.error.generic")}
          </p>
        ) : null,
      )}

      {/*
        The preview is the page: what the assistant built stands wide beside the assistant
        column the shell keeps on the left, from the moment the first pass lands (UI-41,
        UI-42). Until then the preview column shows the building state: the phase, what the
        run is doing and which entities it reads. The template is the model's context, never
        the preview, so the frame never shows a dashboard every run would share (SDK-14).
      */}
      <div className="space-y-4">
        <section aria-labelledby="run-preview" className="space-y-2 rounded border border-border p-2">
          <h2 id="run-preview" className="text-base font-semibold">
            {t("agentRun.preview.title")}
          </h2>
          {record.previewUrl !== undefined && record.previewUrl !== "" ? (
            <>
              <p className="text-sm text-fg-muted">{t("agentRun.preview.hint")}</p>
              {/*
                No `allow-same-origin`: the app is served from the Portal's own origin, and
                that pair beside `allow-scripts` is not a sandbox at all — the frame could
                read the deliberately readable CSRF cookie and write as the signed-in
                reviewer (AP-19). The URL carries the pass number, so a new pass is a new
                frame rather than a stale one. `allow-forms` because an application saves
                through a form: without it the browser drops the submit event, so the SDK is
                never called and Save does nothing, silently (T-1219). The preview's own CSP
                is `form-action 'none'`, so a form still navigates nowhere.
              */}
              <iframe
                ref={frame}
                key={record.previewUrl}
                title={t("agentRun.preview.frameTitle", { app: displayName })}
                src={record.previewUrl}
                sandbox="allow-scripts allow-forms"
                className="h-[82vh] min-h-[28rem] w-full rounded border border-border bg-surface"
              />
              <a
                href={record.previewUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-block text-sm text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
              >
                {t("agentRun.preview.open")}
              </a>
            </>
          ) : (
            <Building
              createdAt={record.createdAt}
              status={record.status}
              over={over}
              events={events}
              dataNeeds={record.dataNeeds}
            />
          )}
        </section>

        <RunTimeline status={record.status} steps={record.steps} tokensUsed={record.tokensUsed} />

        {(record.firstFrameMs != null || record.firstVersionMs != null) && (
          <div className="flex flex-wrap gap-4 text-xs text-fg-muted" data-testid="run-timings">
            {record.firstFrameMs != null ? (
              <span>
                <span className="font-medium text-fg">{t("agentRun.timing.firstFrame")}</span>{" "}
                {t("agentRun.timing.seconds", {
                  seconds: (record.firstFrameMs / 1000).toFixed(1),
                })}
              </span>
            ) : null}
            {record.firstVersionMs != null ? (
              <span>
                <span className="font-medium text-fg">{t("agentRun.timing.firstVersion")}</span>{" "}
                {t("agentRun.timing.seconds", {
                  seconds: (record.firstVersionMs / 1000).toFixed(1),
                })}
              </span>
            ) : null}
          </div>
        )}

        <div className="flex flex-wrap gap-2">
          <Button
            disabled={over || cancel.isPending}
            onClick={() => {
              cancel.mutate();
            }}
          >
            {t("agentRun.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={!publishable || publish.isPending}
            onClick={() => {
              publish.mutate();
            }}
          >
            {t("agentRun.publish")}
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Seconds since `since`, ticking once a second while `running`. */
export function useElapsed(since: string, running: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) {
      return undefined;
    }
    const timer = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(timer);
    };
  }, [running]);
  return Math.max(0, Math.floor((now - Date.parse(since)) / 1000));
}

/** A live run that stopped without a version: it waits for the person's next message. */
const IDLE_STATES = ["interviewing", "previewing", "awaiting_approval"];

/** The entity types the run's data needs name, once each, without the abstract base class. */
export function typesOf(dataNeeds: unknown): string[] {
  const needs: unknown[] = Array.isArray(dataNeeds) ? dataNeeds : [];
  const types = needs.flatMap((need) => {
    const named = (need as { types?: unknown } | null)?.types;
    return Array.isArray(named) ? named.filter((type): type is string => typeof type === "string") : [];
  });
  return [...new Set(types)].filter((type) => type !== "Entity");
}

/** What the agent said last: the step it is on or, after a failed attempt, its errors. */
export function latestThought(events: RunEvent[]): string {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const text = events[i].payload.text;
    if (events[i].kind === "thought" && typeof text === "string" && text.trim() !== "") {
      return text;
    }
  }
  return "";
}

/**
 * The preview column before the first generated version (SDK-14, SDK-15): building, with the
 * phase, the agent's latest step, the types it reads and the clock; stopped without a version,
 * with the agent's errors and how to try again; ended, with what it said last.
 */
function Building({
  createdAt,
  status,
  over,
  events,
  dataNeeds,
}: {
  createdAt: string;
  status: string;
  over: boolean;
  events: RunEvent[];
  dataNeeds: unknown;
}): JSX.Element {
  const { t } = useTranslation();
  const stuck = !over && IDLE_STATES.includes(status);
  const elapsed = useElapsed(createdAt, !over && !stuck);
  const thought = latestThought(events);
  const types = typesOf(dataNeeds);
  return (
    <div
      role="status"
      data-testid="run-building"
      className="flex h-[50vh] min-h-[20rem] flex-col items-center justify-center gap-3 rounded border border-dashed border-border px-6 text-center"
    >
      {stuck ? (
        <>
          <p className="text-lg font-medium">{t("agentRun.preview.notBuilt")}</p>
          <p className="text-sm text-fg-muted">{t("agentRun.preview.retry")}</p>
        </>
      ) : over ? (
        <p className="text-sm text-fg-muted">{t("agentRun.preview.none")}</p>
      ) : (
        <>
          <p className="text-lg font-medium">{t("agentRun.preview.building")}</p>
          {/* A stopped run's state is already the timeline's red line; here it would be said twice. */}
          <p className="text-sm text-fg-muted">{t(`agentRun.states.${status}`, { defaultValue: status })}</p>
          <div
            aria-hidden="true"
            className="h-1 w-48 overflow-hidden rounded-full bg-surface-subtle"
          >
            <div className="h-full w-1/3 rounded-full bg-primary motion-safe:animate-pulse" />
          </div>
          {types.length > 0 ? (
            <p className="text-sm text-fg-muted">{t("agentRun.preview.reading", { types: types.join(", ") })}</p>
          ) : null}
        </>
      )}
      {thought !== "" ? (
        <p
          data-testid="run-building-thought"
          className="max-h-40 max-w-prose overflow-auto whitespace-pre-wrap break-words text-sm"
        >
          {thought}
        </p>
      ) : null}
      {!over && !stuck ? (
        <p className="font-mono text-sm text-fg-muted">{t("agentRun.preview.elapsed", { seconds: elapsed })}</p>
      ) : null}
    </div>
  );
}
