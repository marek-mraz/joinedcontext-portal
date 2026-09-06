import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { LifecycleBadge } from "../components/status/LifecycleBadge";

export function ApprovalsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const list = useQuery({
    queryKey: queryKeys.changes(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes", {
          params: { path: { project } },
        }),
      ),
  });

  if (list.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (list.isError) {
    const message =
      list.error instanceof ApiError
        ? list.error.problem?.detail ?? list.error.message
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <button
          type="button"
          onClick={() => {
            void list.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  const items = list.data.items ?? [];

  if (items.length === 0) {
    return (
      <div className="space-y-4">
        <h1 className="text-xl font-bold">{t("approvals.title")}</h1>
        <p className="text-sm text-surface-fg/70">{t("approvals.empty")}</p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-bold">{t("approvals.title")}</h1>
      <div className="overflow-x-auto rounded border border-border">
        <table className="w-full border-collapse text-left text-sm">
          <caption className="sr-only">{t("approvals.title")}</caption>
          <thead>
            <tr className="border-b border-border bg-surface-subtle text-left">
              <th scope="col" className="px-4 py-2 font-medium">
                {t("approvals.summary")}
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                {t("approvals.lane")}
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                {t("approvals.author")}
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                {t("approvals.created")}
              </th>
              <th scope="col" className="px-4 py-2 font-medium text-right">
                {t("approvals.actions")}
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {items.map((proposal) => {
              const summaryText = t(
                proposal.summary.key,
                proposal.summary.params as Record<string, unknown>,
              );
              const formattedDate = new Intl.DateTimeFormat(locale, {
                dateStyle: "medium",
                timeStyle: "short",
              }).format(new Date(proposal.createdAt));

              return (
                <tr key={proposal.metadata.name} className="hover:bg-surface-subtle/50">
                  <td className="px-4 py-3 font-medium">
                    <Link
                      to="/projects/$project/approvals/$id"
                      params={{ project, id: proposal.metadata.name }}
                      className="text-primary hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus"
                    >
                      {summaryText}
                    </Link>
                    <div className="font-mono text-xs text-surface-fg/60">
                      {proposal.metadata.name}
                    </div>
                  </td>
                  <td className="px-4 py-3">
                    <LifecycleBadge kind="lane" value={proposal.status.lane} />
                  </td>
                  <td className="px-4 py-3 text-surface-fg">
                    {proposal.author.name}
                  </td>
                  <td className="px-4 py-3 text-surface-fg/80">
                    {formattedDate}
                  </td>
                  <td className="px-4 py-3 text-right">
                    <Link
                      to="/projects/$project/approvals/$id"
                      params={{ project, id: proposal.metadata.name }}
                      className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                    >
                      {t("approvals.view")}
                    </Link>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
