import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../api/client";
import { ConversationPanel } from "../pages/apps/ConversationPanel";
import { TERMINAL_STATES, useAgentRun } from "../pages/apps/useAgentRun";
import type { RunEvent } from "../pages/apps/useAgentRun";
import { ModelFileDrop } from "../pages/models/ModelFileDrop";
import { Icon } from "../components/ui/icons";
import {
  dismissNotice,
  isPortalRoute,
  noticeSnapshot,
  onAssistantChange,
  onOpenRequest,
  parseRun,
  rememberPrefill,
  rememberRun,
  runSnapshot,
} from "./state";

/**
 * The assistant, on the right of every page (UI-45, UI-51..UI-53).
 *
 * Renders as a round bubble at the bottom right whenever the panel is closed. When open,
 * renders a 24 rem right-docked panel (full width on small viewports, full screen on toggle).
 * If no run is remembered, shows the empty state with example prompts, a composer, recent
 * conversations, and the model file drop zone. When a run is remembered, connects the live
 * conversation panel.
 */
export function AssistantDock({ project }: { project: string }): JSX.Element | null {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const raw = useSyncExternalStore(onAssistantChange, runSnapshot);
  const run = useMemo(() => parseRun(raw), [raw]);
  const activeProject = run?.project ?? project;
  const navigated = useSyncExternalStore(onAssistantChange, noticeSnapshot);
  const [open, setOpen] = useState(() => Boolean(parseRun(runSnapshot())));
  const [full, setFull] = useState(false);
  // A run remembered after mount (the run page, the Assistant page, a started conversation)
  // opens the panel; closing forgets the run and leaves the bubble.
  const runId = run?.runId ?? null;
  const [shownRun, setShownRun] = useState(runId);
  if (runId !== shownRun) {
    setShownRun(runId);
    if (runId !== null) {
      setOpen(true);
    }
  }
  const handled = useRef(0);

  const [composerMessage, setComposerMessage] = useState("");
  const [startError, setStartError] = useState<string | null>(null);
  const [isStarting, setIsStarting] = useState(false);

  const { run: record, events, streaming, answer, send, cancel } = useAgentRun(
    activeProject,
    run?.runId ?? null,
  );

  useEffect(() => {
    return onOpenRequest(() => {
      setOpen(true);
    });
  }, []);

  useEffect(() => {
    if (!full) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setFull(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [full]);

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

  const recentQuery = useQuery({
    queryKey: ["agent-runs", activeProject, "conversation", "mine"],
    enabled: open && !run,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs", {
          params: {
            path: { project: activeProject },
            query: { kind: "conversation", mine: true },
          },
        }),
      ),
  });

  const liveRecent = useMemo(
    () =>
      (recentQuery.data?.items ?? [])
        .filter((item) => !TERMINAL_STATES.includes(item.status))
        .slice(0, 3),
    [recentQuery.data],
  );

  const startConversation = async (promptText: string) => {
    setStartError(null);
    setIsStarting(true);
    try {
      const created = await unwrap(
        await api.POST("/api/v1/projects/{project}/assistant/conversations", {
          params: { path: { project: activeProject } },
          body: { message: promptText },
        }),
      );
      rememberRun({ project: activeProject, runId: created.id });
      setComposerMessage("");
    } catch (err) {
      const detail =
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : String(err);
      setStartError(detail);
    } finally {
      setIsStarting(false);
    }
  };

  const over = record.data ? TERMINAL_STATES.includes(record.data.status) : false;
  const lastEvent = events.length > 0 ? events[events.length - 1] : undefined;
  const isBusy = Boolean(run && !over && lastEvent && lastEvent.kind === "message");

  const iconButton =
    "rounded p-1.5 text-fg-muted hover:bg-surface-subtle hover:text-fg focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50";

  if (!open) {
    return (
      <button
        type="button"
        aria-expanded={false}
        aria-controls="run-chat"
        aria-label={t("assistant.open")}
        title={t("assistant.open")}
        onClick={() => {
          setOpen(true);
        }}
        className="fixed bottom-4 right-4 z-40 flex size-14 items-center justify-center rounded-full bg-primary text-primary-fg shadow-lg hover:bg-primary-hover focus:outline-none focus:ring-2 focus:ring-border-focus"
      >
        <Icon name="chat" className="size-6" />
        {isBusy ? (
          <span
            data-testid="assistant-busy"
            className="absolute right-1 top-1 size-3 rounded-full bg-danger ring-2 ring-surface"
          />
        ) : null}
      </button>
    );
  }

  return (
    <aside
      aria-label={t("agentRun.conversation.title")}
      className={
        full
          ? "fixed inset-x-0 bottom-0 top-14 z-40 flex flex-col gap-2 bg-surface p-3"
          : "flex w-full shrink-0 flex-col gap-2 border-l border-border bg-surface p-3 md:sticky md:top-14 md:h-[calc(100vh-3.5rem)] md:w-[24rem]"
      }
    >
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">{t("assistant.title")}</h2>
        <div className="flex items-center gap-1">
          {run && !over ? (
            <button
              type="button"
              aria-label={t("assistant.cancel")}
              title={t("assistant.cancel")}
              disabled={cancel.isPending}
              onClick={() => {
                cancel.mutate();
              }}
              className={iconButton}
            >
              <Icon name="stop" className="size-4" />
            </button>
          ) : null}
          <button
            type="button"
            aria-pressed={full}
            aria-label={full ? t("assistant.sideView") : t("assistant.fullScreen")}
            title={full ? t("assistant.sideView") : t("assistant.fullScreen")}
            onClick={() => {
              setFull(!full);
            }}
            className={iconButton}
          >
            <Icon name={full ? "shrink" : "expand"} className="size-4" />
          </button>
          <button
            type="button"
            aria-expanded={open}
            aria-controls="run-chat"
            aria-label={t("assistant.hide")}
            title={t("assistant.hide")}
            onClick={() => {
              setFull(false);
              setOpen(false);
            }}
            className={iconButton}
          >
            <Icon name="minimize" className="size-4" />
          </button>
          <button
            type="button"
            aria-label={t("assistant.close")}
            title={t("assistant.close")}
            onClick={() => {
              rememberRun(null);
              setOpen(false);
            }}
            className={iconButton}
          >
            <Icon name="close" className="size-4" />
          </button>
        </div>
      </div>

      {navigated !== null && isPortalRoute(navigated) ? (
        <div
          role="status"
          className="flex w-full items-center justify-between gap-2 rounded border border-border bg-surface px-3 py-2 text-sm"
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

      {!run ? (
        <div
          id="run-chat"
          data-testid="assistant-empty"
          className="flex min-h-[24rem] w-full flex-1 flex-col gap-4 overflow-y-auto rounded border border-border bg-surface p-3 md:min-h-0"
        >
          <p className="text-sm text-fg-muted">{t("assistant.empty.lead")}</p>
          <div className="flex flex-col gap-2">
            {[
              t("assistant.empty.examples.find"),
              t("assistant.empty.examples.share"),
              t("assistant.empty.examples.build"),
            ].map((exampleText) => (
              <button
                key={exampleText}
                type="button"
                disabled={isStarting}
                onClick={() => {
                  void startConversation(exampleText);
                }}
                className="rounded border border-border bg-surface-subtle p-2 text-left text-xs text-fg hover:bg-surface-muted focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50"
              >
                {exampleText}
              </button>
            ))}
          </div>

          <form
            className="flex flex-col gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              const text = composerMessage.trim();
              if (text && !isStarting) {
                void startConversation(text);
              }
            }}
          >
            <label htmlFor="assistant-empty-composer" className="text-xs font-medium text-fg-muted">
              {t("assistant.empty.composer")}
            </label>
            <textarea
              id="assistant-empty-composer"
              rows={3}
              value={composerMessage}
              placeholder={t("assistant.empty.composer")}
              onChange={(event) => {
                setComposerMessage(event.target.value);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  const text = composerMessage.trim();
                  if (text && !isStarting) {
                    void startConversation(text);
                  }
                }
              }}
              className="block min-w-0 flex-1 resize-none rounded border border-border bg-surface px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus"
            />
            <div className="flex justify-end">
              <button
                type="submit"
                disabled={composerMessage.trim() === "" || isStarting}
                className="rounded bg-primary px-3 py-1.5 text-xs font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
              >
                {t("assistant.empty.send")}
              </button>
            </div>
          </form>

          {startError ? (
            <p role="alert" className="text-xs text-danger">
              {t("assistant.empty.failed")} {startError}
            </p>
          ) : null}

          {liveRecent.length > 0 ? (
            <div className="flex flex-col gap-1.5 border-t border-border pt-2">
              <p className="text-xs font-medium text-fg-muted">{t("assistant.empty.recent")}</p>
              <div className="flex flex-col gap-1">
                {liveRecent.map((r) => {
                  const title = r.prompt
                    ? r.prompt.length > 80
                      ? r.prompt.slice(0, 80)
                      : r.prompt
                    : r.id;
                  return (
                    <button
                      key={r.id}
                      type="button"
                      onClick={() => {
                        rememberRun({ project: activeProject, runId: r.id });
                      }}
                      className="truncate rounded border border-border bg-surface-subtle px-2.5 py-1.5 text-left text-xs text-fg hover:bg-surface-muted focus:outline-none focus:ring-2 focus:ring-border-focus"
                    >
                      {t("assistant.empty.resume", { title })}
                    </button>
                  );
                })}
              </div>
            </div>
          ) : null}
        </div>
      ) : (
        <div
          id="run-chat"
          className="min-h-[24rem] w-full flex-1 rounded border border-border bg-surface md:min-h-0 [&>section]:h-full [&>section]:min-h-0"
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
      )}

      <div className="w-full rounded border border-border bg-surface px-3 py-2">
        <p className="mb-1 text-xs text-fg-muted">{t("assistant.dropHint")}</p>
        <ModelFileDrop
          compact
          project={activeProject}
          onPopulate={(source) => {
            rememberPrefill(`/projects/${activeProject}/models`, { source });
            void navigate({ to: "/projects/$project/models", params: { project: activeProject } });
          }}
        />
      </div>
    </aside>
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
