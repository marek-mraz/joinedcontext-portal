import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, ApiError, unwrap } from "../../api/client";
import { rememberRun, requestOpen } from "../../assistant/state";
import { Button } from "../../components/ui/Button";
import { EmptyState } from "../../components/ui/EmptyState";
import { Field } from "../../components/ui/Field";
import { Input, Select, Textarea } from "../../components/ui/Input";
import { PageHeader } from "../../components/ui/PageHeader";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui/Table";
import { appDisplayName, useEndpointTitles } from "../apps/appTitle";
import { TERMINAL_STATES } from "../apps/useAgentRun";
import { AgentAccess } from "./AgentAccess";

interface RunRecord {
  id: string;
  project: string;
  appName?: string;
  title?: string | null;
  endpointName?: string;
  kind?: string;
  unattended?: boolean;
  continues?: string | null;
  status: string;
  prompt: string;
  createdAt: string;
  firstFrameMs?: number | null;
  firstVersionMs?: number | null;
  error?: string;
}

/** An application reads by its title, or its name as words; a conversation by its first message. */
function formatTitle(run: RunRecord, endpointTitles: Map<string, string>): string {
  if (run.kind && run.kind !== "conversation" && (run.title || run.appName)) {
    return appDisplayName({
      title: run.title,
      appName: run.appName,
      endpointTitle: run.endpointName ? endpointTitles.get(run.endpointName) : undefined,
    });
  }
  return run.prompt.length > 80 ? `${run.prompt.slice(0, 80)}…` : run.prompt;
}

/**
 * The central assistant page listing conversations and autonomous work runs (UI-54, AG-71).
 *
 * Displays all visible runs newest first with lifecycle state, performance timings, and continuation
 * links. Supports interactive filtering by kind, state, and ownership ("mine"). Allows opening active
 * or past runs into the assistant panel and initiating unattended agent work (application, dashboard,
 * analysis) via least-privilege creation flows (UI-55, AG-69).
 */
export function AssistantPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const endpointTitles = useEndpointTitles(project);
  const queryClient = useQueryClient();

  const [kindFilter, setKindFilter] = useState<string>("");
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [mineFilter, setMineFilter] = useState<boolean>(false);

  const [newWorkKind, setNewWorkKind] = useState<string>("application");
  const [newWorkName, setNewWorkName] = useState<string>("");
  const [endpointName, setEndpointName] = useState<string>("");
  const [newWorkPrompt, setNewWorkPrompt] = useState<string>("");
  const [newWorkError, setNewWorkError] = useState<string | null>(null);

  const runsKey = ["projects", project, "agent-runs", { kind: kindFilter, mine: mineFilter }];

  const runsQuery = useQuery({
    queryKey: runsKey,
    queryFn: async () => {
      const result = await api.GET("/api/v1/projects/{project}/agent-runs", {
        params: {
          path: { project },
          query: { limit: 100, kind: kindFilter || undefined, mine: mineFilter || undefined },
        },
      });
      return unwrap(result);
    },
    refetchInterval: (query) => {
      const items = (query.state.data?.items ?? []) as RunRecord[];
      const anyLive = items.some((item) => !TERMINAL_STATES.includes(item.status));
      return anyLive ? 5000 : false;
    },
  });

  const endpointsQuery = useQuery({
    queryKey: ["projects", project, "endpoints"],
    queryFn: async () => {
      const result = await api.GET("/api/v1/projects/{project}/{plural}", {
        params: { path: { project, plural: "endpoints" } },
      });
      return unwrap(result);
    },
  });

  const endpointNames = useMemo(() => {
    const raw = endpointsQuery.data?.items ?? [];
    return raw
      .map((item: unknown) => {
        if (typeof item === "object" && item !== null) {
          if (
            "metadata" in item &&
            typeof (item as { metadata?: { name?: string } }).metadata?.name === "string"
          ) {
            return (item as { metadata: { name: string } }).metadata.name;
          }
          if ("name" in item && typeof (item as { name?: string }).name === "string") {
            return (item as { name: string }).name;
          }
        }
        return "";
      })
      .filter(Boolean);
  }, [endpointsQuery.data?.items]);

  // The first endpoint until the person picks another.
  const chosenEndpoint = endpointName || (endpointNames[0] ?? "");

  const filteredRuns = useMemo(() => {
    const runs = (runsQuery.data?.items ?? []) as RunRecord[];
    return runs.filter((run) => {
      if (statusFilter === "live") {
        return !TERMINAL_STATES.includes(run.status);
      }
      if (statusFilter === "ended") {
        return TERMINAL_STATES.includes(run.status);
      }
      return true;
    });
  }, [runsQuery.data, statusFilter]);

  const continueMutation = useMutation({
    mutationFn: async (priorRunId: string) => {
      const result = await api.POST("/api/v1/projects/{project}/assistant/conversations", {
        params: { path: { project } },
        body: { message: t("assistantPage.continueMessage"), continues: priorRunId },
      });
      return unwrap(result);
    },
    onSuccess: (created) => {
      rememberRun({ project, runId: created.id });
      requestOpen();
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "agent-runs"] });
    },
  });

  const newWorkMutation = useMutation({
    mutationFn: async (payload: {
      appName: string;
      endpointName: string;
      appClass: string;
      visibility: string;
      prompt: string;
      dataNeeds: unknown[];
      kind: string;
      unattended: boolean;
    }) => {
      setNewWorkError(null);
      const result = await api.POST("/api/v1/projects/{project}/agent-runs", {
        params: { path: { project } },
        body: payload as never,
      });
      const created = await unwrap(result);
      return created as { id: string };
    },
    onSuccess: (created) => {
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "agent-runs"] });
      rememberRun({ project, runId: created.id });
      requestOpen();
      setNewWorkName("");
      setNewWorkPrompt("");
    },
    onError: (err) => {
      const detail =
        err instanceof ApiError
          ? (err.problem?.detail ?? err.problem?.title ?? err.message)
          : err instanceof Error
            ? err.message
            : String(err);
      setNewWorkError(detail);
    },
  });

  const handleOpen = (run: RunRecord) => {
    rememberRun({ project, runId: run.id });
    requestOpen();
  };

  const handleStartNewWork = () => {
    const name = newWorkName.trim();
    const prompt = newWorkPrompt.trim();
    const ep = chosenEndpoint;
    if (!name || !prompt) {
      return;
    }
    newWorkMutation.mutate({
      appName: name,
      endpointName: ep,
      appClass: "static",
      visibility: "project",
      prompt,
      dataNeeds: [],
      kind: newWorkKind,
      unattended: true,
    });
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title={t("assistantPage.title")}
        description={t("assistantPage.lead")}
      />

      <div className="flex flex-wrap items-center justify-between gap-4 rounded-lg border border-border bg-surface p-4">
        <div className="flex flex-wrap items-center gap-4">
          <div className="flex items-center gap-2">
            <label htmlFor="filter-kind" className="text-caption font-medium text-fg-muted">
              {t("assistantPage.filters.kind")}
            </label>
            <Select
              id="filter-kind"
              aria-label={t("assistantPage.filters.kind")}
              value={kindFilter}
              onChange={(e) => setKindFilter(e.target.value)}
              className="w-40"
            >
              <option value="">{t("assistantPage.filters.all")}</option>
              <option value="conversation">{t("assistantPage.kinds.conversation")}</option>
              <option value="application">{t("assistantPage.kinds.application")}</option>
              <option value="dashboard">{t("assistantPage.kinds.dashboard")}</option>
              <option value="analysis">{t("assistantPage.kinds.analysis")}</option>
            </Select>
          </div>

          <div className="flex items-center gap-2">
            <label htmlFor="filter-status" className="text-caption font-medium text-fg-muted">
              {t("assistantPage.filters.status")}
            </label>
            <Select
              id="filter-status"
              aria-label={t("assistantPage.filters.status")}
              value={statusFilter}
              onChange={(e) => setStatusFilter(e.target.value)}
              className="w-32"
            >
              <option value="all">{t("assistantPage.filters.all")}</option>
              <option value="live">{t("assistantPage.filters.live")}</option>
              <option value="ended">{t("assistantPage.filters.ended")}</option>
            </Select>
          </div>

          <label className="flex cursor-pointer select-none items-center gap-2 text-body">
            <input
              type="checkbox"
              checked={mineFilter}
              onChange={(e) => setMineFilter(e.target.checked)}
              className="size-4 rounded border-border text-primary focus:ring-border-focus"
            />
            <span className="text-caption font-medium text-fg">
              {t("assistantPage.filters.mine")}
            </span>
          </label>
        </div>
      </div>

      {runsQuery.isPending ? (
        <p role="status" className="text-body text-fg-muted">
          {t("app.loading")}
        </p>
      ) : filteredRuns.length === 0 ? (
        <EmptyState icon="chat" title={t("assistantPage.empty")} />
      ) : (
        <Table caption={t("assistantPage.title")}>
          <TableHead>
            <TableHeaderCell>{t("assistantPage.newWork.name")}</TableHeaderCell>
            <TableHeaderCell>{t("assistantPage.filters.status")}</TableHeaderCell>
            <TableHeaderCell className="hidden md:table-cell">
              {t("assistantPage.timings")}
            </TableHeaderCell>
            <TableHeaderCell className="hidden sm:table-cell">
              {t("assistantPage.started")}
            </TableHeaderCell>
            <TableHeaderCell align="right">{t("assistantPage.actions")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {filteredRuns.map((run) => {
              const ended = TERMINAL_STATES.includes(run.status);
              const isEndedConversation = run.kind === "conversation" && ended;
              const title = formatTitle(run, endpointTitles);
              return (
                <TableRow key={run.id}>
                  <TableCell className="max-w-[28rem]">
                    <div className="flex min-w-0 flex-col gap-0.5">
                      <span className="truncate font-semibold text-fg" title={title}>
                        {title}
                      </span>
                      <span className="text-caption text-fg-muted">
                        <span>
                          {t(`assistantPage.kinds.${run.kind ?? "conversation"}`, {
                            defaultValue: run.kind,
                          })}
                        </span>
                        {run.continues ? (
                          <> · <span className="italic">{t("assistantPage.continues")}</span></>
                        ) : null}
                      </span>
                    </div>
                  </TableCell>
                  <TableCell>
                    <span
                      className={clsx(
                        "inline-flex items-center gap-1.5 whitespace-nowrap rounded px-2 py-0.5 text-caption font-medium",
                        run.status === "failed"
                          ? "bg-danger-soft text-danger"
                          : ended
                            ? "bg-surface-subtle text-fg-muted"
                            : "bg-primary-soft text-primary-soft-fg",
                      )}
                    >
                      {ended ? null : (
                        <span aria-hidden className="size-1.5 rounded-full bg-current" />
                      )}
                      {t(`assistantPage.states.${run.status}`, {
                        defaultValue: t(`agentRun.states.${run.status}`, {
                          defaultValue: run.status,
                        }),
                      })}
                    </span>
                  </TableCell>
                  <TableCell className="hidden whitespace-nowrap text-caption text-fg-muted md:table-cell">
                    {run.firstFrameMs != null || run.firstVersionMs != null ? (
                      <dl className="grid grid-cols-[auto_auto] gap-x-2">
                        {run.firstFrameMs != null ? (
                          <>
                            <dt>{t("agentRun.timing.firstFrame")}</dt>
                            <dd className="text-right tabular-nums">
                              {t("agentRun.timing.seconds", {
                                seconds: (run.firstFrameMs / 1000).toFixed(1),
                              })}
                            </dd>
                          </>
                        ) : null}
                        {run.firstVersionMs != null ? (
                          <>
                            <dt>{t("agentRun.timing.firstVersion")}</dt>
                            <dd className="text-right tabular-nums">
                              {t("agentRun.timing.seconds", {
                                seconds: (run.firstVersionMs / 1000).toFixed(1),
                              })}
                            </dd>
                          </>
                        ) : null}
                      </dl>
                    ) : (
                      "—"
                    )}
                  </TableCell>
                  <TableCell className="hidden whitespace-nowrap text-caption text-fg-muted sm:table-cell">
                    {new Date(run.createdAt).toLocaleString(i18n.language, {
                      dateStyle: "medium",
                      timeStyle: "short",
                    })}
                  </TableCell>
                  <TableCell align="right">
                    <div className="flex items-center justify-end gap-2">
                      {isEndedConversation ? (
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={
                            continueMutation.isPending && continueMutation.variables === run.id
                          }
                          disabled={continueMutation.isPending}
                          onClick={() => continueMutation.mutate(run.id)}
                        >
                          {t("assistantPage.continue")}
                        </Button>
                      ) : null}
                      <Button size="sm" variant="secondary" onClick={() => handleOpen(run)}>
                        {t("assistantPage.open")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      )}

      <section
        aria-labelledby="new-work-heading"
        className="space-y-4 rounded-lg border border-border bg-surface p-5 shadow-1"
      >
        <div className="space-y-1">
          <h2 id="new-work-heading" className="text-lg font-semibold text-fg">
            {t("assistantPage.newWork.title")}
          </h2>
          <p className="text-caption text-fg-muted">
            {t("assistantPage.newWork.unattended")}
          </p>
        </div>

        <form
          onSubmit={(e) => {
            e.preventDefault();
            handleStartNewWork();
          }}
          className="space-y-4"
        >
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
            <Field id="new-work-kind" label={t("assistantPage.newWork.kind")}>
              <Select
                id="new-work-kind"
                value={newWorkKind}
                onChange={(e) => setNewWorkKind(e.target.value)}
              >
                <option value="application">{t("assistantPage.kinds.application")}</option>
                <option value="dashboard">{t("assistantPage.kinds.dashboard")}</option>
                <option value="analysis">{t("assistantPage.kinds.analysis")}</option>
              </Select>
            </Field>

            <Field
              id="new-work-name"
              label={t("assistantPage.newWork.name")}
              help={t("assistantPage.newWork.nameHint")}
            >
              <Input
                id="new-work-name"
                value={newWorkName}
                onChange={(e) => setNewWorkName(e.target.value)}
                placeholder="my-app"
                required
              />
            </Field>

            <Field id="new-work-endpoint" label={t("assistantPage.newWork.endpoint")}>
              <Select
                id="new-work-endpoint"
                value={chosenEndpoint}
                onChange={(e) => setEndpointName(e.target.value)}
                disabled={endpointsQuery.isPending || endpointNames.length === 0}
              >
                {endpointNames.length === 0 ? (
                  <option value="">{t("assistantPage.newWork.endpoint")}</option>
                ) : (
                  endpointNames.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))
                )}
              </Select>
            </Field>
          </div>

          <Field id="new-work-prompt" label={t("assistantPage.newWork.prompt")}>
            <Textarea
              id="new-work-prompt"
              rows={3}
              value={newWorkPrompt}
              onChange={(e) => setNewWorkPrompt(e.target.value)}
              placeholder={t("assistantPage.newWork.prompt")}
              required
            />
          </Field>

          {newWorkError ? (
            <p role="alert" className="text-caption font-medium text-danger">
              {newWorkError}
            </p>
          ) : null}

          <Button
            type="submit"
            variant="primary"
            loading={newWorkMutation.isPending}
            disabled={
              newWorkMutation.isPending ||
              !newWorkName.trim() ||
              !newWorkPrompt.trim()
            }
          >
            {t("assistantPage.newWork.start")}
          </Button>
        </form>
      </section>

      <AgentAccess project={project} />
    </div>
  );
}
