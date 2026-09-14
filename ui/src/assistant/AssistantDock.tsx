import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { JSX } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ConversationPanel } from "../pages/apps/ConversationPanel";
import { TERMINAL_STATES, useAgentRun } from "../pages/apps/useAgentRun";
import type { RunEvent } from "../pages/apps/useAgentRun";
import { ModelFileDrop } from "../pages/models/ModelFileDrop";
import {
  dismissNotice,
  isPortalRoute,
  noticeSnapshot,
  onAssistantChange,
  parseRun,
  rememberPrefill,
  rememberRun,
  runSnapshot,
} from "./state";

/**
 * The assistant, docked to the right edge of every page (UI-41, UI-45).
 *
 * The window belongs to the shell rather than to the run page, so the conversation stays open
 * while the assistant sends the person somewhere else: a `navigate` event switches the route,
 * leaves the prefill for the page to pick up, and says so in a banner. A route that is not a
 * path inside the Portal is ignored here as it is refused on the server, so a frame the Portal
 * did not write moves nothing.
 */
export function AssistantDock(): JSX.Element | null {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // The run is an external store (session storage, written by the run page): subscribed and
  // read through React's own seam, so a run remembered before this dock mounted is not missed.
  const raw = useSyncExternalStore(onAssistantChange, runSnapshot);
  const run = useMemo(() => parseRun(raw), [raw]);
  const navigated = useSyncExternalStore(onAssistantChange, noticeSnapshot);
  const [open, setOpen] = useState(true);
  const handled = useRef(0);

  const { run: record, events, streaming, answer, send } = useAgentRun(
    run?.project ?? "",
    run?.runId ?? null,
  );

  useEffect(() => {
    const next = events.find((event) => event.kind === "navigate" && event.seq > handled.current);
    if (!next) {
      return;
    }
    handled.current = next.seq;
    const route = routeOf(next);
    if (route === null) {
      return;
    }
    const draft = next.payload.draft as { kind?: string; name?: string } | undefined;
    const targetRoute =
      draft?.name && typeof draft.name === "string"
        ? `${route}${route.includes("?") ? "&" : "?"}draft=${encodeURIComponent(draft.name)}`
        : route;
    rememberPrefill(route, prefillOf(next));
    void navigate({ to: targetRoute as "/" });
  }, [events, navigate]);

  if (!run) {
    return null;
  }
  const over = record.data ? TERMINAL_STATES.includes(record.data.status) : false;

  return (
    <div className="fixed bottom-4 left-4 z-30 flex w-[22rem] max-w-[calc(100vw-2rem)] flex-col items-start gap-2">
      {navigated !== null && isPortalRoute(navigated) ? (
        <div
          role="status"
          className="flex w-full items-center justify-between gap-2 rounded border border-border bg-surface px-3 py-2 text-sm shadow"
        >
          <span>{t("assistant.navigated", { route: navigated })}</span>
          <button
            type="button"
            onClick={() => {
              dismissNotice();
            }}
            className="rounded px-2 py-0.5 text-fg-muted hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("assistant.dismiss")}
          </button>
        </div>
      ) : null}
      {open ? (
        <div className="w-full rounded border border-border bg-surface px-3 py-2 shadow">
          <ModelFileDrop
            compact
            project={run.project}
            onPopulate={(source) => {
              // The models page picks the draft up as a prefill, the way every assistant
              // navigation hands a page its form (DM-54, CC-71).
              rememberPrefill(`/projects/${run.project}/models`, { source });
              void navigate({ to: "/projects/$project/models", params: { project: run.project } });
            }}
          />
        </div>
      ) : null}
      <div className="flex gap-2">
        <button
          type="button"
          aria-expanded={open}
          aria-controls="run-chat"
          onClick={() => {
            setOpen((current) => !current);
          }}
          className="rounded-full border border-border bg-surface px-4 py-1.5 text-sm shadow hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {open ? t("agentRun.chat.hide") : t("agentRun.chat.show")}
        </button>
        <button
          type="button"
          onClick={() => {
            rememberRun(null);
          }}
          className="rounded-full border border-border bg-surface px-3 py-1.5 text-sm shadow hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("assistant.close")}
        </button>
      </div>
      <div
        id="run-chat"
        hidden={!open}
        className="h-[80vh] w-full rounded border border-border bg-surface shadow-lg [&>section]:h-full [&>section]:min-h-0"
      >
        <ConversationPanel
          project={run.project}
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
      </div>
    </div>
  );
}

function routeOf(event: RunEvent): string | null {
  const route = event.payload.route;
  return isPortalRoute(route) ? route : null;
}

function prefillOf(event: RunEvent): Record<string, unknown> {
  const prefill = event.payload.prefill;
  return typeof prefill === "object" && prefill !== null && !Array.isArray(prefill)
    ? (prefill as Record<string, unknown>)
    : {};
}
