import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { isOwn } from "../api/approval";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { useAuth } from "../auth/AuthProvider";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import {
  Alert,
  Button,
  EmptyState,
  Icon,
  PageHeader,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
  buttonClass,
} from "../components/ui";

const COLUMNS = 6;

export function ApprovalsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const { identity } = useAuth();
  const [mine, setMine] = useState(false);
  const [phase, setPhase] = useState("");

  const list = useQuery({
    queryKey: queryKeys.changes(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes", {
          params: { path: { project } },
        }),
      ),
  });

  const header = <PageHeader title={t("approvals.title")} description={t("approvals.lead")} />;

  const head = (
    <TableHead>
      <TableHeaderCell>{t("approvals.summary")}</TableHeaderCell>
      <TableHeaderCell>{t("approvals.phase")}</TableHeaderCell>
      <TableHeaderCell>{t("approvals.lane")}</TableHeaderCell>
      <TableHeaderCell>{t("approvals.author")}</TableHeaderCell>
      <TableHeaderCell>{t("approvals.created")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
    </TableHead>
  );

  if (list.isPending) {
    return (
      <div className="flex flex-col gap-section">
        {header}
        <Table caption={t("approvals.title")} status={t("app.loading")}>
          {head}
          <TableSkeleton columns={COLUMNS} />
        </Table>
      </div>
    );
  }

  if (list.isError) {
    const message =
      list.error instanceof ApiError
        ? (list.error.problem?.detail ?? list.error.message)
        : t("app.error.generic");
    return (
      <div className="flex flex-col gap-section">
        {header}
        <Alert
          role="alert"
          tone="danger"
          actions={
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() => {
                void list.refetch();
              }}
            >
              {t("app.error.retry")}
            </Button>
          }
        >
          {message}
        </Alert>
      </div>
    );
  }

  const all = list.data.items ?? [];
  // The phases the list holds, in the order a change goes through them.
  const phases = [...new Set(all.map((proposal) => proposal.status.phase))];
  // Both filters narrow what the API already let this caller read; they decide nothing.
  const items = all.filter(
    (proposal) =>
      (!mine || isOwn(identity?.email, proposal)) && (phase === "" || proposal.status.phase === phase),
  );
  const filters =
    all.length > 0 ? (
      <div className="flex flex-wrap items-center gap-4">
        <label className="flex items-center gap-2 text-body">
          <input type="checkbox" checked={mine} onChange={(event) => setMine(event.target.checked)} />
          {t("approvals.filterMine")}
        </label>
        <label className="flex items-center gap-2 text-body">
          {t("approvals.filterPhase")}
          <Select value={phase} onChange={(event) => setPhase(event.target.value)}>
            <option value="">{t("approvals.filterAll")}</option>
            {phases.map((value) => (
              <option key={value} value={value}>
                {t(`phase.${value.charAt(0).toLowerCase()}${value.slice(1)}`, { defaultValue: value })}
              </option>
            ))}
          </Select>
        </label>
      </div>
    ) : null;

  if (all.length === 0) {
    return (
      <div className="flex flex-col gap-section">
        {header}
        <EmptyState icon="approvals" title={t("approvals.empty")} />
      </div>
    );
  }
  if (items.length === 0) {
    return (
      <div className="flex flex-col gap-section">
        {header}
        {filters}
        <EmptyState icon="approvals" title={t("approvals.noneMatch")} />
      </div>
    );
  }

  const dateFormatter = new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  });

  return (
    <div className="flex flex-col gap-section">
      {header}
      {filters}
      <Table caption={t("approvals.title")}>
        {head}
        <TableBody>
          {items.map((proposal) => {
            const summaryText = t(
              proposal.summary.key,
              proposal.summary.params as Record<string, unknown>,
            );
            const formattedDate = dateFormatter.format(new Date(proposal.createdAt));

            return (
              <TableRow key={proposal.metadata.name}>
                <TableCell primary>
                  <Link
                    to="/projects/$project/approvals/$id"
                    params={{ project, id: proposal.metadata.name }}
                    className="focus-ring rounded-sm text-primary hover:underline"
                  >
                    {summaryText}
                  </Link>
                  <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                    {proposal.metadata.name}
                  </div>
                  {/* A bundle is more than its headline, and the approval checks all of it. */}
                  {(proposal.fileCount ?? 0) > 1 ? (
                    <div className="mt-0.5 text-caption text-fg-muted">
                      {t("approvals.fileCount", { count: proposal.fileCount })}
                    </div>
                  ) : null}
                </TableCell>
                <TableCell>
                  <LifecycleBadge kind="phase" value={proposal.status.phase} />
                </TableCell>
                <TableCell>
                  <LifecycleBadge kind="lane" value={proposal.status.lane} />
                </TableCell>
                <TableCell>{proposal.author.name}</TableCell>
                <TableCell className="whitespace-nowrap text-fg-muted">{formattedDate}</TableCell>
                <TableCell align="right">
                  <Link
                    to="/projects/$project/approvals/$id"
                    params={{ project, id: proposal.metadata.name }}
                    className={buttonClass("secondary", "sm")}
                  >
                    {t("approvals.view")}
                    <Icon name="chevronRight" className="size-3.5" />
                  </Link>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}
