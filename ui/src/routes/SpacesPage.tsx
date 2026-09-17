import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../api/permissions";
import { Link } from "@tanstack/react-router";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, isChange, localized, refName } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ChangeNotice } from "../components/ChangeNotice";
import { ProjectQuota, useProjectUsage } from "../components/ProjectQuota";
import { DeleteResourceAction } from "../components/DeleteResourceDialog";
import { EditResourceAction } from "../components/EditResourceDialog";
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
  SourceLink,
} from "../components/ui";

interface SpaceForm {
  name: string;
  title?: string;
  dataModelRef?: string;
  defaultLocale?: string;
  isSandbox?: boolean;
  ttlDays?: number;
}

function toEnvelope(project: string, form: SpaceForm) {
  const { name, title, ...spec } = form;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: {
      name,
      namespace: project,
      ...(title?.trim() ? { title } : {}),
    },
    spec,
  };
}

const COLUMNS = 5;

/** Context Spaces of one project: what exists, what it costs against the quota, where it lives. */
export function SpacesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const usage = useProjectUsage(project);
  const [dialogOpen, setDialogOpen] = useState(false);
  const mayPropose = usePermissions(project).can("ContextSpace", "propose");
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
      <TableHeaderCell secondary>{t("spaces.field.dataModel")}</TableHeaderCell>
      <TableHeaderCell>{t("spaces.field.phase")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("spaces.field.inside")}</TableHeaderCell>
      <TableHeaderCell align="right" secondary>{t("spaces.field.source")}</TableHeaderCell>
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
  // The API counts what the project holds and knows which quota is in force, the project's own
  // or the organization's default (PF-73, PF-75); the page only reads the numbers.
  const contextSpaces = usage.data?.contextSpaces;
  const limit = contextSpaces?.limit;
  const quotaExceeded = limit !== undefined && (contextSpaces?.used ?? spaces.length) >= limit;

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("spaces.title")}
        description={t("spaces.lead")}
        actions={
          mayPropose ? (
            <div className="flex items-center gap-2">
              <Link
                to="/projects/$project/spaces/complete"
                params={{ project }}
                className={buttonClass("secondary", "md")}
              >
                {t("spaces.complete.title")}
              </Link>
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
            </div>
          ) : null
        }
      />

      <ProjectQuota project={project} />

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
                dataModelRef?: unknown;
                isSandbox?: boolean;
                ttlDays?: number;
              };
              const model = refName(spec.dataModelRef);
              const title = localized(space.metadata.title, locale, space.metadata.name);
              const target = {
                project,
                kind: "ContextSpace",
                plural: "spaces",
                name: space.metadata.name,
                label: title,
              };
              return (
                <TableRow key={space.metadata.name}>
                  <TableCell primary>
                    <div>{title}</div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-1.5 font-mono text-caption text-fg-subtle">
                      {space.metadata.title ? <span>{space.metadata.name}</span> : null}
                      {spec.isSandbox ? (
                        <Badge tone="warning">{t("spaces.sandbox", { days: spec.ttlDays ?? 0 })}</Badge>
                      ) : null}
                    </div>
                  </TableCell>
                  <TableCell secondary>
                    {model ? (
                      <span className="font-mono text-caption">{model}</span>
                    ) : (
                      <span className="text-fg-subtle">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge kind="phase" value={space.status?.phase} />
                  </TableCell>
                  <TableCell align="right">
                    <div className="flex items-center justify-end gap-1.5">
                      <Link
                        to="/projects/$project/spaces/$name"
                        params={{ project, name: space.metadata.name }}
                        className={buttonClass("secondary", "sm")}
                      >
                        {t("spaces.inside.open")}
                      </Link>
                      <EditResourceAction target={target} />
                      <DeleteResourceAction target={target} />
                    </div>
                  </TableCell>
                  <TableCell align="right" secondary>
                    {space.status?.sourceUrl ? (
                      <SourceLink href={space.status.sourceUrl} label={t("spaces.field.source")} />
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
        kind="ContextSpace"
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
