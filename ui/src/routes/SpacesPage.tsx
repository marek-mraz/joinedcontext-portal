import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ChangeNotice } from "../components/ChangeNotice";
import { contextSpaceSchema } from "../schemas/kinds";

interface SpaceForm {
  name: string;
  title?: Record<string, string>;
  dataModelRef?: string;
  defaultLocale?: string;
  isSandbox?: boolean;
  ttlDays?: number;
}

interface Quotas {
  contextSpaces?: number;
  residentPipelines?: number;
  publicEndpoints?: number;
  ingestEventsPerSecond?: number;
}

function toEnvelope(project: string, form: SpaceForm) {
  const { name, title, ...spec } = form;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: {
      name,
      namespace: project,
      ...(title && Object.keys(title).length > 0 ? { title } : {}),
    },
    spec,
  };
}

/** How much of a quota is used, as a labelled bar rather than a colour alone. */
function QuotaBar({
  label,
  used,
  limit,
}: {
  label: string;
  used: number;
  limit?: number;
}): JSX.Element {
  const { t } = useTranslation();
  const percent = limit && limit > 0 ? Math.min(100, Math.round((used / limit) * 100)) : 0;
  return (
    <div className="min-w-[12rem]">
      <div className="flex items-baseline justify-between gap-2 text-sm">
        <span className="font-medium">{label}</span>
        <span className="font-mono text-xs text-surface-fg/70">
          {limit === undefined ? t("quota.unlimited", { used }) : `${used} / ${limit}`}
        </span>
      </div>
      {limit === undefined ? null : (
        <div
          role="progressbar"
          aria-valuenow={used}
          aria-valuemin={0}
          aria-valuemax={limit}
          aria-label={label}
          className="mt-1 h-2 w-full overflow-hidden rounded bg-surface-subtle"
        >
          <div
            className={percent >= 100 ? "h-full bg-danger" : "h-full bg-primary"}
            style={{ width: `${percent}%` }}
          />
        </div>
      )}
    </div>
  );
}

/** Context Spaces of one project: what exists, what it costs against the quota, where it lives. */
export function SpacesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const [dialogOpen, setDialogOpen] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });

  // The quota lives on the Project manifest; a project without one is simply unlimited.
  const projectQuery = useQuery({
    queryKey: queryKeys.resource(project, "projects", project),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "projects", name: project } },
        }),
      ),
  });

  const create = useMutation({
    mutationFn: async (form: SpaceForm) => {
      setFormError(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
          body: toEnvelope(project, form) as never,
        }),
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "spaces") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  if (list.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (list.isError) {
    const message =
      list.error instanceof ApiError
        ? (list.error.problem?.detail ?? list.error.message)
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

  const spaces = asManifests(list.data.items ?? []);
  const quotas = (projectQuery.data?.spec as { quotas?: Quotas } | undefined)?.quotas;
  const limit = quotas?.contextSpaces;
  const quotaExceeded = limit !== undefined && spaces.length >= limit;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <h1 className="text-xl font-bold">{t("spaces.title")}</h1>
        <div className="flex flex-wrap items-end gap-6">
          <QuotaBar label={t("quota.contextSpaces")} used={spaces.length} limit={limit} />
          <button
            type="button"
            disabled={quotaExceeded}
            onClick={() => {
              setFormError(null);
              setDialogOpen(true);
            }}
            className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {t("spaces.add")}
          </button>
        </div>
      </div>

      {quotaExceeded ? (
        <p role="status" className="text-sm text-danger">
          {t("quota.exceeded", { limit })}
        </p>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}

      {spaces.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("spaces.empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left text-sm">
            <caption className="sr-only">{t("spaces.title")}</caption>
            <thead>
              <tr className="border-b border-border bg-surface-subtle">
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("spaces.field.name")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("spaces.field.dataModel")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("spaces.field.phase")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium text-right">
                  {t("spaces.field.inside")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium text-right">
                  {t("spaces.field.source")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {spaces.map((space: Manifest) => {
                const spec = space.spec as {
                  dataModelRef?: string;
                  isSandbox?: boolean;
                  ttlDays?: number;
                };
                return (
                  <tr key={space.metadata.name} className="hover:bg-surface-subtle/50">
                    <td className="px-4 py-3">
                      <div className="font-medium">
                        {localized(space.metadata.title, locale, space.metadata.name)}
                      </div>
                      <div className="font-mono text-xs text-surface-fg/60">
                        {space.metadata.title ? space.metadata.name : null}
                        {spec.isSandbox
                          ? `${space.metadata.title ? " · " : ""}${t("spaces.sandbox", { days: spec.ttlDays ?? 0 })}`
                          : ""}
                      </div>
                    </td>
                    <td className="px-4 py-3">{spec.dataModelRef ?? "—"}</td>
                    <td className="px-4 py-3">
                      <LifecycleBadge kind="phase" value={space.status?.phase} />
                    </td>
                    <td className="px-4 py-3 text-right">
                      <Link
                        to="/projects/$project/spaces/$name"
                        params={{ project, name: space.metadata.name }}
                        className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                      >
                        {t("spaces.inside.open")}
                      </Link>
                    </td>
                    <td className="px-4 py-3 text-right">
                      {space.status?.sourceUrl ? (
                        <a
                          href={space.status.sourceUrl}
                          target="_blank"
                          rel="noreferrer"
                          className="text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
                        >
                          {t("spaces.field.source")}
                        </a>
                      ) : (
                        <span className="text-surface-fg/40">—</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <ResourceFormDialog<SpaceForm>
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("spaces.add")}
        description={t("spaces.addHint")}
        schema={contextSpaceSchema(t)}
        submitLabel={t("spaces.propose")}
        disabled={create.isPending}
        error={formError}
        onSubmit={(form) => create.mutate(form)}
      />
    </div>
  );
}
