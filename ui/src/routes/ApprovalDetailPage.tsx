import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { useAuth } from "../auth/AuthProvider";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { PlanDiffViewer } from "../components/diff/PlanDiffViewer";
import type { components } from "../api/schema";

type ChangeProposal = components["schemas"]["ChangeProposal"];

/**
 * The word a red-lane approval has to be typed back. It is the resource name the summary
 * names, which is exactly what the API compares `confirm` against (CC-19, CC-39).
 */
function computeExpectedName(proposal: ChangeProposal): string {
  const params = proposal.summary.params as Record<string, unknown>;
  return typeof params.name === "string" ? params.name : proposal.metadata.name;
}

export function ApprovalDetailPage({
  project,
  id,
}: {
  project: string;
  id: string;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { identity, hasRole } = useAuth();

  const [confirmInput, setConfirmInput] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);

  const detailQuery = useQuery({
    queryKey: queryKeys.change(project, id),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/{id}", {
          params: { path: { project, id } },
        }),
      ),
    // Deploying is the one phase the reconciler moves on without the user acting.
    refetchInterval: (query) =>
      query.state.data?.status.phase === "Deploying" ? 5_000 : false,
  });

  const proposal = detailQuery.data;

  const isRedLane = proposal?.status.lane === "red";
  const expectedName = proposal ? computeExpectedName(proposal) : "";
  const confirmMatches = !isRedLane || confirmInput.trim() === expectedName;

  const hasApproverRole = hasRole("portal-approver");
  const isAuthor = Boolean(
    identity?.email &&
      proposal?.author.email &&
      identity.email.toLowerCase() === proposal.author.email.toLowerCase(),
  );

  let disabledReason: string | null = null;
  if (!hasApproverRole) {
    disabledReason = t("approvals.needsRole");
  } else if (isAuthor) {
    disabledReason = t("approvals.ownProposal");
  }

  const approveMutation = useMutation({
    mutationFn: async () => {
      setActionError(null);
      const res = await api.POST("/api/v1/projects/{project}/changes/{id}/approve", {
        params: { path: { project, id } },
        body: isRedLane ? { confirm: confirmInput.trim() } : undefined,
      });
      return unwrap(res);
    },
    // The 202 carries the change with its new phase, so the chip flips without a refetch.
    onSuccess: (change) => {
      queryClient.setQueryData(queryKeys.change(project, id), (prev?: ChangeProposal) =>
        prev ? { ...prev, status: change.status } : prev,
      );
      // `exact`, or the prefix match would also refetch this change and undo the line above.
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project), exact: true });
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        setActionError(err.problem?.detail ?? err.message);
      } else if (err instanceof Error) {
        setActionError(err.message);
      } else {
        setActionError(t("app.error.generic"));
      }
    },
  });

  const rejectMutation = useMutation({
    mutationFn: async () => {
      setActionError(null);
      const res = await api.POST("/api/v1/projects/{project}/changes/{id}/reject", {
        params: { path: { project, id } },
      });
      return unwrap(res);
    },
    onSuccess: (change) => {
      queryClient.setQueryData(queryKeys.change(project, id), (prev?: ChangeProposal) =>
        prev ? { ...prev, status: change.status } : prev,
      );
      // `exact`, or the prefix match would also refetch this change and undo the line above.
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project), exact: true });
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        setActionError(err.problem?.detail ?? err.message);
      } else if (err instanceof Error) {
        setActionError(err.message);
      } else {
        setActionError(t("app.error.generic"));
      }
    },
  });

  if (detailQuery.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (detailQuery.isError) {
    const message =
      detailQuery.error instanceof ApiError
        ? detailQuery.error.problem?.detail ?? detailQuery.error.message
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <button
          type="button"
          onClick={() => {
            void detailQuery.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  // `proposal` was read before the guards above, so TypeScript still has it as optional here.
  // A settled, non-error query always carries data; this is the check that says so.
  if (!proposal) {
    return <p role="status">{t("app.loading")}</p>;
  }

  const isMutating = approveMutation.isPending || rejectMutation.isPending;
  const isPendingApproval = proposal.status.phase === "PendingApproval";
  const canAct = !disabledReason && isPendingApproval && !isMutating;
  const canApprove = canAct && confirmMatches;
  const canReject = canAct;

  const summaryText = t(
    proposal.summary.key,
    proposal.summary.params as Record<string, unknown>,
  );

  return (
    <div className="space-y-6">
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-xl font-bold">{summaryText}</h1>
          <LifecycleBadge kind="lane" value={proposal.status.lane} />
          <LifecycleBadge kind="phase" value={proposal.status.phase} />
        </div>
        <p className="font-mono text-sm text-surface-fg/60">{proposal.metadata.name}</p>
      </div>

      <dl className="grid grid-cols-1 gap-4 rounded border border-border bg-surface-subtle p-4 sm:grid-cols-2">
        <div>
          <dt className="text-xs font-medium text-surface-fg/70">{t("approvals.author")}</dt>
          <dd className="mt-1 text-sm font-medium text-surface-fg">
            {proposal.author.name}
            {proposal.author.email ? (
              <span className="ml-1 text-xs text-surface-fg/60">({proposal.author.email})</span>
            ) : null}
          </dd>
        </div>
        {proposal.status.mergeRequest ? (
          <div>
            <dt className="text-xs font-medium text-surface-fg/70">
              {t("approvals.mergeRequest")}
            </dt>
            <dd className="mt-1 text-sm font-medium">
              <a
                href={proposal.status.mergeRequest}
                target="_blank"
                rel="noreferrer"
                className="text-primary hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus"
              >
                {proposal.status.mergeRequest}
              </a>
            </dd>
          </div>
        ) : null}
      </dl>

      <section aria-labelledby="plan-diff-heading" className="space-y-2">
        <h2 id="plan-diff-heading" className="text-base font-semibold text-surface-fg">
          {t("approvals.diffType")}
        </h2>
        <PlanDiffViewer fields={proposal.planFields} />
      </section>

      {actionError ? (
        <div role="alert" className="rounded border border-danger bg-danger/10 p-3 text-sm text-danger">
          {actionError}
        </div>
      ) : null}

      <section aria-labelledby="actions-heading" className="space-y-4 rounded border border-border p-4">
        <h2 id="actions-heading" className="sr-only">
          {t("approvals.actions")}
        </h2>

        {isRedLane && isPendingApproval ? (
          <div className="space-y-1">
            <label htmlFor="confirm-resource-name" className="block text-sm font-medium text-surface-fg">
              {t("approvals.confirmPrompt")}{" "}
              <span className="font-mono font-semibold">{expectedName}</span>
            </label>
            <input
              id="confirm-resource-name"
              type="text"
              value={confirmInput}
              onChange={(e) => setConfirmInput(e.target.value)}
              aria-label={t("approvals.confirmLabel")}
              placeholder={expectedName}
              disabled={!canAct}
              className="block w-full max-w-sm rounded border border-border bg-surface px-3 py-1.5 text-sm text-surface-fg placeholder:text-surface-fg/40 focus:border-border-focus focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
            />
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            disabled={!canApprove}
            onClick={() => approveMutation.mutate()}
            className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {approveMutation.isPending ? t("approvals.approving") : t("approvals.approve")}
          </button>

          <button
            type="button"
            disabled={!canReject}
            onClick={() => rejectMutation.mutate()}
            className="inline-flex items-center justify-center rounded border border-danger bg-surface px-4 py-2 text-sm font-medium text-danger hover:bg-danger/10 focus:outline-none focus:ring-2 focus:ring-danger focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {rejectMutation.isPending ? t("approvals.rejecting") : t("approvals.reject")}
          </button>

          {disabledReason ? (
            <p className="text-sm text-surface-fg/70">{disabledReason}</p>
          ) : null}
        </div>
      </section>
    </div>
  );
}
