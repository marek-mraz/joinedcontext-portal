import { useState } from "react";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { proposeChecked } from "../api/proposal";
import { asManifests, isChange, localized, refName } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ResourceList } from "../components/ResourceList";
import { ChangeNotice } from "../components/ChangeNotice";
import { ProjectQuota, useProjectUsage } from "../components/ProjectQuota";
import { DeleteResourceAction } from "../components/DeleteResourceDialog";
import { EditResourceAction } from "../components/EditResourceDialog";
import { SaveAsResourceAction } from "../components/SaveAsDialog";
import { WorkOnCopyAction } from "../components/WorkOnCopyDialog";
import type { ResourceTarget } from "../components/DeleteResourceDialog";
import { usePermissions } from "../api/permissions";
import { RowActions } from "../components/ui/RowActions";
import type { RowAction } from "../components/ui/RowActions";
import { contextSpaceSchema } from "../schemas/kinds";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
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
  /** Pins the `{space}` segment of a space that predates PF-84; kept as it is on edit. */
  urnSegment?: string;
}

/** The `{space}` segment the space's entity ids carry: its pin, else `{project}-{name}` (PF-84). */
export function spaceSegment(project: string, name: string, pin?: string): string {
  return pin ?? `${project}-${name}`;
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

/** A draft or the YAML view read back into the form: the envelope's name, title and spec. */
function fromEnvelope(manifest: unknown, locale = "en"): SpaceForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string; title?: unknown }; spec?: object };
  // A manifest's title is a locale map (MF-01), and the form has one box: the box reads the box's
  // language. ponytail: a title in several languages collapses to the one shown if the person
  // edits it, which is what one Title box has always meant here.
  const title = localized(
    envelope.metadata?.title as string | Record<string, string> | undefined,
    locale,
    "",
  );
  return {
    ...(envelope.spec ?? {}),
    name: envelope.metadata?.name ?? "",
    ...(title === "" ? {} : { title }),
  } as SpaceForm;
}

const COLUMNS = 5;

/** Context Spaces of one project: what exists, what it costs against the quota, where it lives. */
/**
 * What a person can do with one space: Open in the row, the rest behind its menu (T-2279, UI-26).
 *
 * The dialogs are rendered here and not inside the menu, because a menu unmounts when it closes and
 * would take an open dialog with it; each action component is asked for its dialog alone
 * (`trigger={false}`) and the row says when it is open. An action the person's role cannot take is
 * not listed at all, the way the permission guard used to hide its button.
 */
function SpaceRowActions({
  project,
  name,
  target,
  locale,
}: {
  project: string;
  name: string;
  target: ResourceTarget;
  locale: string;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(target.home ?? project);
  const [openAction, setOpenAction] = useState<"edit" | "saveAs" | "copy" | "delete" | null>(null);
  const opens = (action: "edit" | "saveAs" | "copy" | "delete") => (open: boolean) =>
    setOpenAction(open ? action : null);

  // Every action is listed, and one a role cannot take carries the reason instead of vanishing: that
  // is what the permission guard did for the buttons, and UI-44 does not stop at a menu's edge.
  const denied = (verb: "propose" | "delete") =>
    permissions.can(target.kind, verb)
      ? undefined
      : t("permissions.denied", { verb, kind: target.kind });
  const actions: RowAction[] = [
    {
      key: "edit",
      label: t("resourceEdit.button"),
      onSelect: () => setOpenAction("edit"),
      disabledReason: denied("propose"),
    },
    {
      key: "saveAs",
      label: t("saveAs.button"),
      onSelect: () => setOpenAction("saveAs"),
      disabledReason: denied("propose"),
    },
    {
      key: "copy",
      label: t("workspaces.open.action"),
      onSelect: () => setOpenAction("copy"),
    },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setOpenAction("delete"),
      disabledReason: denied("delete"),
    },
  ];

  return (
    <>
      <RowActions
        label={target.label ?? name}
        actions={actions}
        primary={
          <Link
            to="/projects/$project/spaces/$name"
            params={{ project, name }}
            className={buttonClass("secondary", "sm")}
          >
            {t("spaces.inside.open")}
          </Link>
        }
      />
      {/* The kind's own form, not the manifest as text: the same schema and envelope the create
          dialog uses (T-2278, UI-61). */}
      <EditResourceAction
        target={target}
        trigger={false}
        open={openAction === "edit"}
        onOpenChange={opens("edit")}
        form={{
          schema: contextSpaceSchema(t),
          fromManifest: (manifest) => fromEnvelope(manifest, locale) as unknown as Record<string, unknown>,
          toManifest: (edited) => toEnvelope(project, edited as unknown as SpaceForm),
        }}
      />
      <SaveAsResourceAction
        target={target}
        trigger={false}
        open={openAction === "saveAs"}
        onOpenChange={opens("saveAs")}
      />
      <WorkOnCopyAction
        project={project}
        scope={{ kind: "space", name }}
        trigger={false}
        open={openAction === "copy"}
        onOpenChange={opens("copy")}
      />
      <DeleteResourceAction
        target={target}
        trigger={false}
        open={openAction === "delete"}
        onOpenChange={opens("delete")}
      />
    </>
  );
}

export function SpacesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const usage = useProjectUsage(project);
  // The assistant may have sent the person here with a new space already drafted (`?draft=`,
  // AG-45, UI-45): the dialog opens on it and the form loads the draft by its name, the way the
  // data sources and pipelines pages do.
  const [urlDraftName] = useState(() =>
    typeof window === "undefined"
      ? undefined
      : (new URLSearchParams(window.location.search).get("draft") ?? undefined),
  );
  const [dialogOpen, setDialogOpen] = useState(urlDraftName !== undefined);
  // The dialog is controlled: its draft, its YAML view and its check all read what it holds.
  const [form, setForm] = useState<SpaceForm | undefined>(undefined);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
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
      return proposeChecked(
        project,
        "spaces",
        toEnvelope(project, form) as { metadata: { name: string } },
        true,
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

  const spaces = asManifests(list.data?.items ?? []);
  // The API counts what the project holds and knows which quota is in force, the project's own
  // or the organization's default (PF-73, PF-75); the page only reads the numbers.
  const contextSpaces = usage.data?.contextSpaces;
  const limit = contextSpaces?.limit;
  const quotaExceeded = limit !== undefined && (contextSpaces?.used ?? spaces.length) >= limit;

  // The same control in the header and in the empty list, disabled with the reason for a viewer.
  const addButton = (
    <PermissionGuard project={project} kind="ContextSpace" verb="propose">
      <Button
        variant="primary"
        disabled={quotaExceeded}
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => {
          setFormError(null);
          setForm(undefined);
          setDialogOpen(true);
        }}
      >
        {t("spaces.add")}
      </Button>
    </PermissionGuard>
  );

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("spaces.title")}
        description={t("spaces.lead")}
        actions={
          <div className="flex items-center gap-2">
            <PermissionGuard project={project} kind="ContextSpace" verb="propose">
              <Link
                to="/projects/$project/spaces/complete"
                params={{ project }}
                className={buttonClass("secondary", "md")}
              >
                {t("spaces.complete.title")}
              </Link>
            </PermissionGuard>
            {addButton}
          </div>
        }
      />


      {quotaExceeded ? (
        <Alert role="status" tone="warning">
          {t("quota.exceeded", { limit })}
        </Alert>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <ResourceList
        query={list}
        caption={t("spaces.title")}
        head={head}
        columns={COLUMNS}
        count={spaces.length}
        empty={
          <EmptyState
            bare
            icon="spaces"
            title={t("spaces.empty")}
            description={t("spaces.addHint")}
            action={addButton}
          />
        }
      >
        {spaces.map((space: Manifest) => {
          const spec = space.spec as {
            dataModelRef?: unknown;
            isSandbox?: boolean;
            ttlDays?: number;
            urnSegment?: string;
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
                  <span>
                    {t("spaces.segment", {
                      segment: spaceSegment(project, space.metadata.name, spec.urnSegment),
                    })}
                  </span>
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
                <SpaceRowActions
                  project={project}
                  name={space.metadata.name}
                  target={target}
                  locale={locale}
                />
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
        })}
      </ResourceList>

      {/* What the project holds against its quota comes after what it holds: for a first
          steward the list, empty or not, is the page, and the bar is secondary (T-1396). */}
      <ProjectQuota project={project} />

      <ResourceFormDialog<SpaceForm>
        kind="ContextSpace"
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("spaces.add")}
        description={t("spaces.addHint")}
        schema={contextSpaceSchema(t)}
        // The same YAML view, draft, Check and verdict every other form has (T-1380, PF-57):
        // strict validation refuses a space nobody checked, so the form checks it first.
        formData={form}
        onChange={setForm}
        project={project}
        draftKind="ContextSpace"
        draftName={urlDraftName}
        plural="spaces"
        source={{
          toManifest: (form) => toEnvelope(project, form),
          fromManifest: (manifest) => fromEnvelope(manifest, locale),
        }}
        submitLabel={t("spaces.propose")}
        submitting={create.isPending}
        error={formError}
        onSubmit={(form) => create.mutate(form)}
      />
    </div>
  );
}
