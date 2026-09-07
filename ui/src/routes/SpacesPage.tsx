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
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
  buttonClass,
} from "../components/ui";

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
      <div className="flex items-baseline justify-between gap-3 text-caption">
        <span className="font-medium text-fg-muted">{label}</span>
        <span className="font-mono tabular-nums text-fg">
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
          className="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-neutral-200"
        >
          <div
            className={percent >= 100 ? "h-full rounded-full bg-danger" : "h-full rounded-full bg-primary"}
            style={{ width: `${percent}%` }}
          />
        </div>
      )}
    </div>
  );
}

const COLUMNS = 5;

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

  const head = (
    <TableHead>
      <TableHeaderCell>{t("spaces.field.name")}</TableHeaderCell>
      <TableHeaderCell>{t("spaces.field.dataModel")}</TableHeaderCell>
      <TableHeaderCell>{t("spaces.field.phase")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("spaces.field.inside")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("spaces.field.source")}</TableHeaderCell>
    </TableHead>
  );

  if (list.isPending) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("spaces.title")} description={t("spaces.lead")} />
        <Table caption={t("spaces.title")} status={t("app.loading")}>
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
        <PageHeader title={t("spaces.title")} description={t("spaces.lead")} />
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

  const spaces = asManifests(list.data.items ?? []);
  const quotas = (projectQuery.data?.spec as { quotas?: Quotas } | undefined)?.quotas;
  const limit = quotas?.contextSpaces;
  const quotaExceeded = limit !== undefined && spaces.length >= limit;

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("spaces.title")}
        description={t("spaces.lead")}
        aside={<QuotaBar label={t("quota.contextSpaces")} used={spaces.length} limit={limit} />}
        actions={
          <Button
            variant="primary"
            disabled={quotaExceeded}
            icon={<Icon name="plus" className="size-4" />}
            onClick={() => {
              setFormError(null);
              setDialogOpen(true);
            }}
          >
            {t("spaces.add")}
          </Button>
        }
      />

      {quotaExceeded ? (
        <Alert role="status" tone="warning">
          {t("quota.exceeded", { limit })}
        </Alert>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <Table caption={t("spaces.title")}>
        {head}
        <TableBody>
          {spaces.length === 0 ? (
            <TableEmpty columns={COLUMNS}>
              <EmptyState bare icon="spaces" title={t("spaces.empty")} description={t("spaces.addHint")} />
            </TableEmpty>
          ) : (
            spaces.map((space: Manifest) => {
              const spec = space.spec as {
                dataModelRef?: string;
                isSandbox?: boolean;
                ttlDays?: number;
              };
              return (
                <TableRow key={space.metadata.name}>
                  <TableCell primary>
                    <div>{localized(space.metadata.title, locale, space.metadata.name)}</div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-1.5 font-mono text-caption text-fg-subtle">
                      {space.metadata.title ? <span>{space.metadata.name}</span> : null}
                      {spec.isSandbox ? (
                        <Badge tone="warning">{t("spaces.sandbox", { days: spec.ttlDays ?? 0 })}</Badge>
                      ) : null}
                    </div>
                  </TableCell>
                  <TableCell>
                    {spec.dataModelRef ? (
                      <span className="font-mono text-caption">{spec.dataModelRef}</span>
                    ) : (
                      <span className="text-fg-subtle">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge kind="phase" value={space.status?.phase} />
                  </TableCell>
                  <TableCell align="right">
                    <Link
                      to="/projects/$project/spaces/$name"
                      params={{ project, name: space.metadata.name }}
                      className={buttonClass("secondary", "sm")}
                    >
                      {t("spaces.inside.open")}
                    </Link>
                  </TableCell>
                  <TableCell align="right">
                    {space.status?.sourceUrl ? (
                      <a
                        href={space.status.sourceUrl}
                        target="_blank"
                        rel="noreferrer"
                        className={buttonClass("ghost", "sm", "text-primary")}
                      >
                        {t("spaces.field.source")}
                        <Icon name="external" className="size-3.5" />
                      </a>
                    ) : (
                      <span className="text-fg-subtle">—</span>
                    )}
                  </TableCell>
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>

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
