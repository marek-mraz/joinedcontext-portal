import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { approvalStanding, changedKind } from "../../api/approval";
import { usePermissions } from "../../api/permissions";
import { useIdentity } from "../../auth/AuthProvider";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { buttonClass, SourceLink } from "../../components/ui";

/**
 * Where an application lives and how it goes live (AP-71): its source in Git, and after Publish
 * the Change that publishes it, approved right here by a person the approval rules allow
 * (CC-34, PF-50, PF-58). A red-lane Change is approved on its own page, where its name is typed
 * back (CC-39).
 */
export function RunPublication({
  project,
  sourceUrl,
  changeId,
  onApproved,
}: {
  project: string;
  sourceUrl?: string;
  changeId?: string;
  onApproved?: () => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const identity = useIdentity();
  const permissions = usePermissions(project);

  const change = useQuery({
    queryKey: queryKeys.change(project, changeId ?? ""),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/{id}", {
          params: { path: { project, id: changeId ?? "" } },
        }),
      ),
    enabled: Boolean(changeId),
    refetchInterval: (query) =>
      query.state.data?.status.phase === "Deploying" ? 5_000 : false,
  });

  const approve = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/changes/{id}/approve", {
          params: { path: { project, id: changeId ?? "" } },
        }),
      ),
    onSuccess: (approved) => {
      queryClient.setQueryData(queryKeys.change(project, changeId ?? ""), (prev: unknown) =>
        prev && typeof prev === "object" ? { ...prev, status: approved.status } : prev,
      );
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project), exact: true });
      onApproved?.();
    },
  });

  if (!sourceUrl && !changeId) {
    return null;
  }

  const proposal = change.data;
  const pending = proposal?.status.phase === "PendingApproval";
  const red = proposal?.status.lane === "red";
  const standing = proposal
    ? approvalStanding(permissions, identity?.email ?? undefined, proposal)
    : null;

  return (
    <section
      aria-label={t("agentRun.publication.title")}
      className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 rounded border border-border p-3 text-sm"
    >
      {sourceUrl ? (
        <SourceLink href={sourceUrl} label={t("agentRun.publication.source")} />
      ) : null}

      {changeId ? (
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          <span className="font-medium">{t("agentRun.publication.title")}</span>
          <span className="font-mono text-xs text-fg-muted">{changeId}</span>
          {proposal ? (
            <>
              <LifecycleBadge kind="lane" value={proposal.status.lane} />
              <LifecycleBadge kind="phase" value={proposal.status.phase} />
            </>
          ) : null}
        </span>
      ) : null}

      {changeId && pending && standing ? (
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          {standing.block === null && !red ? (
            <button
              type="button"
              disabled={approve.isPending}
              onClick={() => approve.mutate()}
              className={buttonClass("primary", "sm")}
            >
              {approve.isPending ? t("approvals.approving") : t("agentRun.publication.approve")}
            </button>
          ) : null}
          <Link
            to="/projects/$project/approvals/$id"
            params={{ project, id: changeId }}
            className={buttonClass("secondary", "sm")}
          >
            {t("changes.review")}
          </Link>
          {standing.ownAsAdministrator ? (
            <span className="text-xs text-fg-muted">
              {t("approvals.ownAsAdministrator", { kind: proposal ? changedKind(proposal) : "App" })}
            </span>
          ) : null}
          {standing.block ? (
            <span className="text-xs text-fg-muted">{t(`approvals.${standing.block}`)}</span>
          ) : null}
        </span>
      ) : null}

      {approve.error ? (
        <p role="alert" className="basis-full text-danger">
          {approve.error instanceof ApiError
            ? (approve.error.problem?.detail ?? approve.error.message)
            : t("app.error.generic")}
        </p>
      ) : null}
    </section>
  );
}
