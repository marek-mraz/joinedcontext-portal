import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../api/client";
import { Card } from "./ui";

/** One dimension as the API answers it: what the project holds, and what it may. */
interface Dimension {
  used: number;
  limit?: number;
}

/** What the project holds of each quota, as the API counts it (PF-75). */
export function useProjectUsage(project: string) {
  return useQuery({
    queryKey: ["projectUsage", project],
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}", { params: { path: { project } } })),
    enabled: project !== "",
    retry: false,
    select: (detail) => (detail.status?.usage ?? {}) as Record<string, Dimension>,
  });
}

/**
 * What the project holds of each quota (PF-73, PF-75). The numbers are the API's own
 * (`GET /api/v1/projects/{project}`), so a person sees the limit before the verdict refuses a
 * change for exceeding it. A caller who may not read the project is answered `404` and the card
 * is simply not there.
 */
export function ProjectQuota({ project }: { project: string }): JSX.Element | null {
  const { t } = useTranslation();
  const detail = useProjectUsage(project);
  const rows = Object.entries(detail.data ?? {});
  if (detail.isPending || detail.isError || rows.length === 0) {
    return null;
  }

  return (
    <Card>
      <h2 className="text-title font-semibold text-fg">{t("quota.title")}</h2>
      <p className="mt-0.5 text-body text-fg-muted">{t("quota.lead")}</p>
      <dl className="mt-4 grid gap-4 sm:grid-cols-2">
        {rows.map(([dimension, { used, limit }]) => {
          const full = typeof limit === "number" && used >= limit;
          return (
            <div key={dimension}>
              <div className="flex items-baseline justify-between gap-2">
                <dt className="text-body text-fg">
                  {t(`quota.dimension.${dimension}`, { defaultValue: dimension })}
                </dt>
                <dd className={`text-body tabular-nums ${full ? "font-semibold text-danger" : "text-fg-muted"}`}>
                  {typeof limit === "number"
                    ? t("quota.ofLimit", { used, limit })
                    : t("quota.noLimit", { used })}
                </dd>
              </div>
              {typeof limit === "number" ? (
                <div
                  role="progressbar"
                  aria-label={t(`quota.dimension.${dimension}`, { defaultValue: dimension })}
                  aria-valuenow={used}
                  aria-valuemin={0}
                  aria-valuemax={limit}
                  className="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-surface-muted"
                >
                  <div
                    className={`h-full rounded-full ${full ? "bg-danger" : "bg-primary"}`}
                    style={{ width: `${Math.min(100, Math.round((used / limit) * 100))}%` }}
                  />
                </div>
              ) : null}
            </div>
          );
        })}
      </dl>
    </Card>
  );
}
