import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import type { JSX } from "react";
import { clsx } from "clsx";
import { useQuery } from "@tanstack/react-query";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../api/client";
import { ConversationPanel } from "../pages/apps/ConversationPanel";
import { TERMINAL_STATES, useAgentRun } from "../pages/apps/useAgentRun";
import type { RunEvent } from "../pages/apps/useAgentRun";
import { ModelFileDrop } from "../pages/models/ModelFileDrop";
import { AppGenerator } from "../pages/apps/AppGenerator";
import { appDisplayName, useEndpointTitles } from "../pages/apps/appTitle";
import {
  DataBar,
  MAX_ENDPOINTS,
  rememberEndpoints,
  runEndpointNames,
  sameEndpoints,
  storedEndpoints,
} from "./EndpointPicker";
import { Icon } from "../components/ui/icons";
import { pageOf } from "./pageOf";
import type { IconName } from "../components/ui/icons";
import {
  dismissNotice,
  isPortalRoute,
  navigatedSeq,
  noticeSnapshot,
  onAssistantChange,
  onOpenRequest,
  parseRun,
  rememberNavigated,
  rememberPrefill,
  rememberRun,
  runSnapshot,
  settleNotice,
  settlePrefill,
  trail,
} from "./state";

/**
 * The assistant, on the right of every page (UI-45, UI-51..UI-53).
 *
 * Renders as a round bubble at the bottom right whenever the panel is closed. When open,
 * renders a 24 rem right-docked panel (full width on small viewports, full screen on toggle).
 * If no run is remembered, shows the empty state with example prompts, a composer, recent
 * conversations, and a paperclip in the composer that drafts a data model from a sample file.
 * When a run is remembered, connects the live conversation panel. Open, it sits beside the page,
 * floats over it, or fills the screen; the choice lasts for the tab.
 */
/** A stable array between renders: `useSyncExternalStore` compares by identity. */
let trailCache: string[] = [];
function trailSnapshot(): string[] {
  const next = trail();
  if (next.length !== trailCache.length || next.some((route, i) => route !== trailCache[i])) {
    trailCache = next;
  }
  return trailCache;
}

export function AssistantDock({ project }: { project: string }): JSX.Element | null {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const raw = useSyncExternalStore(onAssistantChange, runSnapshot);
  const run = useMemo(() => parseRun(raw), [raw]);
  const activeProject = run?.project ?? project;
  const navigated = useSyncExternalStore(onAssistantChange, noticeSnapshot);
  // The pages the assistant opened, so the person walks back without losing the conversation.
  const opened = useSyncExternalStore(onAssistantChange, trailSnapshot);
  // The notice goes once the person moves on from the page it names.
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  useEffect(() => {
    settleNotice(pathname);
    settlePrefill(pathname);
  }, [navigated, pathname]);
  const [open, setOpen] = useState(() => Boolean(parseRun(runSnapshot())));
  const [building, setBuilding] = useState(false);
  const [layout, setLayout] = useState<Layout>(storedLayout);
  useEffect(() => {
    try {
      sessionStorage.setItem(LAYOUT_KEY, layout);
    } catch {
      // No storage (a private window): the layout lasts until the next page.
    }
  }, [layout]);
  const full = layout === "full";
  // A run remembered after mount (the run page, the Assistant page, a started conversation)
  // opens the panel; closing forgets the run and leaves the bubble.
  const runId = run?.runId ?? null;
  const [shownRun, setShownRun] = useState(runId);
  // A change made in a running conversation's data bar waits for its next message (AG-75).
  const [pendingEndpoints, setPendingEndpoints] = useState<string[] | null>(null);
  if (runId !== shownRun) {
    setShownRun(runId);
    setPendingEndpoints(null);
    if (runId !== null) {
      setOpen(true);
    }
  }

  const [composerMessage, setComposerMessage] = useState("");
  const [startError, setStartError] = useState<string | null>(null);
  const [isStarting, setIsStarting] = useState(false);
  // The endpoints the next conversation may query (AG-75), remembered per project for the tab.
  const [chosenEndpoints, setChosenEndpoints] = useState<string[]>(() => storedEndpoints(project));
  const [endpointsOf, setEndpointsOf] = useState(activeProject);
  if (endpointsOf !== activeProject) {
    setEndpointsOf(activeProject);
    setChosenEndpoints(storedEndpoints(activeProject));
  }
  const chooseEndpoints = (names: string[]) => {
    setChosenEndpoints(names);
    rememberEndpoints(activeProject, names);
  };

  const { run: record, events, streaming, answer, send, cancel } = useAgentRun(
    activeProject,
    run?.runId ?? null,
  );

  useEffect(() => {
    return onOpenRequest((intent) => {
      setOpen(true);
      setBuilding(intent === "build");
    });
  }, []);

  useEffect(() => {
    if (!full) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setLayout("side");
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [full]);

  useEffect(() => {
    if (runId === null) {
      return;
    }
    const followed = navigatedSeq(runId);
    const next = events.filter((event) => event.kind === "navigate" && event.seq > followed).at(-1);
    if (!next) {
      return;
    }
    rememberNavigated(runId, next.seq);
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
    // An `href` is parsed into the path and the search: a query inside `to` reaches the address
    // bar but not the router's search, so a page already open is not remounted for it (T-0770).
    void navigate({ href: targetRoute });
  }, [events, navigate, runId]);

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
          // `endpointNames` is AG-75; the generated body type catches up with the next API render.
          body: { message: promptText, endpointNames: chosenEndpoints } as { message: string },
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
  const endpointTitles = useEndpointTitles(activeProject);
  // A run that builds an application says which one, by its title, never by its id.
  const buildingApp = record.data?.appName
    ? appDisplayName({
        title: record.data.title,
        appName: record.data.appName,
        endpointTitle: endpointTitles.get(record.data.endpointName),
      })
    : "";
  const lastEvent = events.length > 0 ? events[events.length - 1] : undefined;
  const isBusy = Boolean(run && !over && lastEvent && lastEvent.kind === "message");

  const iconButton =
    "rounded p-1.5 text-fg-muted hover:bg-surface-subtle hover:text-fg focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50 aria-pressed:bg-primary-soft aria-pressed:text-primary-soft-fg";

  const attach = (
    <ModelFileDrop
      icon
      project={activeProject}
      onPopulate={(source) => {
        rememberPrefill(`/projects/${activeProject}/models`, { source });
        void navigate({ to: "/projects/$project/models", params: { project: activeProject } });
      }}
    />
  );

  // What the running conversation queries: the newest `endpoints` event, else the run record.
  const lastEndpointsEvent = [...events].reverse().find((event) => event.kind === "endpoints");
  const runEndpoints = lastEndpointsEvent
    ? runEndpointNames(
        (Array.isArray(lastEndpointsEvent.payload.names) ? lastEndpointsEvent.payload.names : []).map(
          (name: unknown) => ({ name }),
        ),
      )
    : runEndpointNames((record.data as { endpoints?: unknown } | undefined)?.endpoints);
  // Once the run queries what the bar shows, the change has landed.
  if (pendingEndpoints !== null && sameEndpoints(pendingEndpoints, runEndpoints)) {
    setPendingEndpoints(null);
  }
  const liveEndpoints = pendingEndpoints ?? runEndpoints;
  const addEndpoint = (name: string) => {
    if (!liveEndpoints.includes(name) && liveEndpoints.length < MAX_ENDPOINTS) {
      setPendingEndpoints([...liveEndpoints, name]);
    }
  };
  const liveBar = run ? (
    <DataBar project={run.project} selected={liveEndpoints} onChange={setPendingEndpoints} />
  ) : null;

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
      data-layout={layout}
      className={
        full
          ? "fixed inset-x-0 bottom-0 top-14 z-40 flex flex-col gap-2 bg-surface p-3"
          : layout === "float"
            ? clsx(
                "fixed bottom-4 right-4 z-40 flex h-[min(40rem,calc(100vh-5rem))] w-[calc(100vw-2rem)] flex-col gap-2 rounded-lg border border-border bg-surface p-3 shadow-xl",
                // The app builder is a form: it gets the room a form needs.
                building ? "max-w-[44rem]" : "max-w-[26rem]",
              )
            : clsx(
                "flex w-full shrink-0 flex-col gap-2 border-l border-border bg-surface p-3 md:sticky md:top-14 md:h-[calc(100vh-3.5rem)]",
                building ? "md:w-[40rem]" : "md:w-[24rem]",
              )
      }
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-baseline gap-2">
          <h2 className="shrink-0 text-sm font-semibold">{t("assistant.title")}</h2>
          {run && buildingApp ? (
            <span data-testid="assistant-app" title={record.data?.appName} className="truncate text-xs text-fg-muted">
              {buildingApp}
            </span>
          ) : null}
        </div>
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
          {LAYOUTS.map(({ value, icon, label }) => (
            <button
              key={value}
              type="button"
              aria-pressed={layout === value}
              aria-label={t(label)}
              title={t(label)}
              onClick={() => {
                setLayout(value);
              }}
              className={iconButton}
            >
              <Icon name={icon} className="size-4" />
            </button>
          ))}
          <button
            type="button"
            aria-expanded={open}
            aria-controls="run-chat"
            aria-label={t("assistant.hide")}
            title={t("assistant.hide")}
            onClick={() => {
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
          <span>{t("assistant.navigated", { page: pageOf(navigated, t) })}</span>
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

      {opened.length > 1 ? (
        <nav
          aria-label={t("assistant.trail")}
          data-testid="assistant-trail"
          className="flex w-full flex-wrap items-center gap-2 px-1 text-caption text-fg-muted"
        >
          {opened.map((route) => (
            <button
              key={route}
              type="button"
              onClick={() => {
                void navigate({ href: route });
              }}
              className="rounded px-2 py-0.5 hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {pageOf(route, t)}
            </button>
          ))}
        </nav>
      ) : null}

      {building ? (
        <div
          id="run-chat"
          data-testid="assistant-build"
          className="flex min-h-[24rem] w-full flex-1 flex-col gap-3 overflow-y-auto rounded border border-border bg-surface p-3 md:min-h-0"
        >
          <button
            type="button"
            onClick={() => {
              setBuilding(false);
            }}
            className="self-start rounded border border-border px-2.5 py-1 text-xs hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("assistant.backToChat")}
          </button>
          <AppGenerator
            project={activeProject}
            onStarted={(runId) => {
              setBuilding(false);
              rememberRun({ project: activeProject, runId });
            }}
          />
        </div>
      ) : !run ? (
        <div
          id="run-chat"
          data-testid="assistant-empty"
          className="flex min-h-[24rem] w-full flex-1 flex-col gap-4 overflow-y-auto rounded border border-border bg-surface p-3 md:min-h-0"
        >
          <p className="text-sm text-fg-muted">{t("assistant.empty.lead")}</p>
          <button
            type="button"
            onClick={() => {
              setBuilding(true);
            }}
            className="flex items-center gap-2 rounded border border-border bg-primary-soft p-2 text-left text-sm font-medium text-primary-soft-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            <Icon name="apps" className="size-4" />
            {t("apps.generate.title")}
          </button>
          <div className="flex flex-col gap-2">
            {/* A prompt the caller's role cannot carry out stays, disabled with the reason (T-1390, UI-44). */}
            {(
              [
                ["find", null],
                ["share", "Endpoint"],
                ["build", "Dashboard"],
              ] as const
            ).map(([example, kind]) => {
              const exampleText = t(`assistant.empty.examples.${example}`);
              const button = (
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
              );
              return kind ? (
                <PermissionGuard key={exampleText} project={activeProject} kind={kind} verb="propose">
                  {button}
                </PermissionGuard>
              ) : (
                button
              );
            })}
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
            <DataBar
              project={activeProject}
              selected={chosenEndpoints}
              onChange={chooseEndpoints}
              opens="down"
            />
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
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-1">
                {attach}
              </div>
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
            building={Boolean(record.data?.appName)}
            onAnswer={(questionId, answers) => {
              answer.mutate({ questionId, answers });
            }}
            onSend={(text) => {
              send.mutate(
                pendingEndpoints !== null && !sameEndpoints(pendingEndpoints, runEndpoints)
                  ? { text, endpointNames: pendingEndpoints }
                  : text,
              );
            }}
            attach={attach}
            above={liveBar}
            onUseEndpoint={addEndpoint}
            usedEndpoints={liveEndpoints}
          />
        </div>
      )}

    </aside>
  );
}

/** Beside the page, floating over it, or the whole screen; the bubble is the fourth, closed state. */
type Layout = "side" | "float" | "full";

const LAYOUT_KEY = "jc.assistant.layout";

const LAYOUTS: { value: Layout; icon: IconName; label: string }[] = [
  { value: "side", icon: "sidebar", label: "assistant.sideView" },
  { value: "float", icon: "float", label: "assistant.floatView" },
  { value: "full", icon: "expand", label: "assistant.fullScreen" },
];

function storedLayout(): Layout {
  try {
    const stored = sessionStorage.getItem(LAYOUT_KEY);
    return stored === "float" || stored === "full" ? stored : "side";
  } catch {
    return "side";
  }
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
