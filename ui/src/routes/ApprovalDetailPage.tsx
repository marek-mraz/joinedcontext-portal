import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { approvalStanding, changedKind } from "../api/approval";
import { usePermissions } from "../api/permissions";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { useAuth } from "../auth/AuthProvider";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { PlanDiffViewer } from "../components/diff/PlanDiffViewer";
import type { components } from "../api/schema";
import { Alert, Button, Dialog, Input, PageHeader, Textarea } from "../components/ui";

type ChangeProposal = components["schemas"]["ChangeProposal"];
type ChangeFile = components["schemas"]["ChangeFile"];

/** The list page of the changed resource: the folder of its file, `projects/{project}/{plural}/…`. */
function pluralOf(change: ChangeProposal): string | undefined {
  const kind = changedKind(change);
  const file = (change.files ?? []).find((f) => f.kind === kind) ?? change.files?.[0];
  const parts = file?.path.split("/") ?? [];
  return parts[0] === "projects" && parts.length > 3 ? parts[2] : undefined;
}

/** The order the files are read in: what the strictest lane decides comes first. */
const LANE_ORDER: Record<ChangeFile["lane"], number> = { red: 2, yellow: 1, green: 0 };

/** The three words the diff already uses for what a change does to a file. */
const OPERATION_LABEL: Record<ChangeFile["operation"], string> = {
  Create: "approvals.diffAdded",
  Update: "approvals.diffChanged",
  Delete: "approvals.diffRemoved",
};

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
  const { identity } = useAuth();
  const permissions = usePermissions(project);

  const approveReasonId = useId();
  const rejectReasonId = useId();
  const [rejecting, setRejecting] = useState(false);
  const [rejectReason, setRejectReason] = useState("");
  const [confirmInput, setConfirmInput] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  // The file whose field diff is shown; none picked shows the headline file (T-1397).
  const [selectedPath, setSelectedPath] = useState<string | null>(null);

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

  // What the approval did, said once the phase moves while the page is open (T-1386): the page
  // polls through Deploying, so the move to Applied is seen here too.
  const [banner, setBanner] = useState<"deploying" | "live" | null>(null);
  const phase = proposal?.status.phase;
  const [seenPhase, setSeenPhase] = useState(phase);
  if (phase !== seenPhase) {
    setSeenPhase(phase);
    if (seenPhase !== undefined && phase === "Deploying") setBanner("deploying");
    else if (seenPhase !== undefined && phase === "Applied") setBanner("live");
  }

  const isRedLane = proposal?.status.lane === "red";
  const expectedName = proposal ? computeExpectedName(proposal) : "";
  const confirmMatches = !isRedLane || confirmInput.trim() === expectedName;

  // Approve on the change's kind, and on someone else's change unless the caller administers
  // that kind (PF-50, PF-58); the API checks the same.
  const standing = proposal
    ? approvalStanding(permissions, identity?.email ?? undefined, proposal)
    : { block: null, ownAsAdministrator: false };
  const disabledReason = standing.block ? t(`approvals.${standing.block}`) : null;

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
    mutationFn: async (reason: string) => {
      setActionError(null);
      const res = await api.POST("/api/v1/projects/{project}/changes/{id}/reject", {
        params: { path: { project, id } },
        // The reason goes into the merge request's closing comment, where the proposer reads it.
        body: { reason: reason.trim() } as never,
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
      <Alert
        role="alert"
        tone="danger"
        actions={
          <Button
            size="sm"
            onClick={() => {
              void detailQuery.refetch();
            }}
          >
            {t("app.error.retry")}
          </Button>
        }
      >
        {message}
      </Alert>
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
  // Why Approve is disabled, on the button itself and not only beside it (UI-44).
  const approveReason =
    disabledReason ?? (canAct && !confirmMatches ? t("approvals.confirmFirst", { name: expectedName }) : null);
  const canReject = canAct;

  const summaryText = t(
    proposal.summary.key,
    proposal.summary.params as Record<string, unknown>,
  );

  // Red first: the file that decides the confirmation is the one an approver has to read,
  // and a bundle's headline is rarely it (T-0861).
  const files = [...(proposal.files ?? [])].sort(
    (one, other) => LANE_ORDER[other.lane] - LANE_ORDER[one.lane],
  );
  const selected = files.find((file) => file.path === selectedPath && file.fields);

  return (
    <div className="space-y-6">
      <PageHeader
        title={summaryText}
        description={<span className="font-mono">{proposal.metadata.name}</span>}
        aside={
          <div className="flex flex-wrap items-center gap-2">
            <LifecycleBadge kind="lane" value={proposal.status.lane} />
            <LifecycleBadge kind="phase" value={proposal.status.phase} />
          </div>
        }
      />

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

      {files.length > 0 ? (
        <section aria-labelledby="files-heading" className="space-y-2">
          <h2 id="files-heading" className="text-base font-semibold text-surface-fg">
            {t("approvals.files")} ({t("approvals.fileCount", { count: files.length })})
          </h2>
          <p className="text-caption text-fg-muted">{t("approvals.filesLead")}</p>
          <ul className="divide-y divide-border rounded border border-border">
            {files.map((file) => (
              <li
                key={file.path}
                className="flex flex-wrap items-center gap-2 px-3 py-2"
                data-testid="change-file"
              >
                <LifecycleBadge kind="lane" value={file.lane} />
                {file.kind ? (
                  <span className="text-sm font-medium text-surface-fg">{file.kind}</span>
                ) : null}
                {file.fields ? (
                  <button
                    type="button"
                    aria-pressed={selected?.path === file.path}
                    onClick={() => setSelectedPath(selected?.path === file.path ? null : file.path)}
                    className="font-mono text-caption text-primary hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus"
                  >
                    {file.path}
                  </button>
                ) : (
                  <span className="font-mono text-caption text-fg-subtle">{file.path}</span>
                )}
                <span className="ml-auto text-caption text-fg-muted">
                  {t(OPERATION_LABEL[file.operation])}
                </span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <section aria-labelledby="plan-diff-heading" className="space-y-2">
        <h2 id="plan-diff-heading" className="text-base font-semibold text-surface-fg">
          {selected ? t("approvals.diffForFile", { path: selected.path }) : t("approvals.diffType")}
        </h2>
        <PlanDiffViewer fields={selected ? selected.fields : proposal.planFields} />
      </section>

      {banner && proposal ? (
        <Alert role="status" tone={banner === "live" ? "success" : "info"}>
          <span>
            {t(banner === "live" ? "approvals.approvedLive" : "approvals.approvedDeploying", {
              kind: changedKind(proposal),
            })}
          </span>
          {banner === "live" && pluralOf(proposal) ? (
            <Link
              to="/projects/$project/$plural"
              params={{ project, plural: pluralOf(proposal) as string }}
              className="ml-2 font-medium text-primary hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {t("approvals.openResource", { kind: changedKind(proposal) })}
            </Link>
          ) : null}
        </Alert>
      ) : null}

      {actionError ? (
        <Alert role="alert" tone="danger">
          {actionError}
        </Alert>
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
            <Input
              id="confirm-resource-name"
              value={confirmInput}
              onChange={(e) => setConfirmInput(e.target.value)}
              aria-label={t("approvals.confirmLabel")}
              placeholder={expectedName}
              disabled={!canAct}
              className="max-w-sm"
            />
          </div>
        ) : null}

        {standing.ownAsAdministrator && isPendingApproval ? (
          <p role="note" className="text-sm text-fg-muted">
            {t("approvals.ownAsAdministrator", { kind: changedKind(proposal) })}
          </p>
        ) : null}

        <div className="flex flex-wrap items-center gap-3">
          <span
            title={approveReason ?? undefined}
            tabIndex={approveReason ? 0 : undefined}
            className="inline-flex"
          >
            <Button
              variant="primary"
              disabled={!canApprove}
              aria-describedby={approveReason ? approveReasonId : undefined}
              loading={approveMutation.isPending}
              onClick={() => approveMutation.mutate()}
            >
              {approveMutation.isPending ? t("approvals.approving") : t("approvals.approve")}
            </Button>
          </span>

          <Button
            variant="danger"
            disabled={!canReject}
            loading={rejectMutation.isPending}
            onClick={() => setRejecting(true)}
          >
            {rejectMutation.isPending ? t("approvals.rejecting") : t("approvals.reject")}
          </Button>
          <Dialog
            open={rejecting}
            onOpenChange={setRejecting}
            title={t("approvals.rejectTitle")}
            description={t("approvals.rejectLead")}
            closeLabel={t("approvals.rejectCancel")}
            footer={
              <>
                <Button onClick={() => setRejecting(false)}>{t("approvals.rejectCancel")}</Button>
                <Button
                  variant="danger"
                  disabled={rejectReason.trim() === ""}
                  loading={rejectMutation.isPending}
                  onClick={() => {
                    setRejecting(false);
                    rejectMutation.mutate(rejectReason);
                  }}
                >
                  {t("approvals.rejectConfirm")}
                </Button>
              </>
            }
          >
            <label htmlFor={rejectReasonId} className="text-sm font-medium">
              {t("approvals.rejectReason")}
            </label>
            <Textarea
              id={rejectReasonId}
              rows={3}
              maxLength={2000}
              required
              aria-describedby={`${rejectReasonId}-hint`}
              value={rejectReason}
              onChange={(event) => setRejectReason(event.target.value)}
            />
            <p id={`${rejectReasonId}-hint`} className="text-sm text-fg-muted">
              {t("approvals.rejectReasonHint")}
            </p>
          </Dialog>

          {approveReason ? (
            <p id={approveReasonId} className="text-body text-fg-muted">
              {approveReason}
            </p>
          ) : null}
        </div>
      </section>
    </div>
  );
}
