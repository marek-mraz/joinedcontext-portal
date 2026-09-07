import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, prune } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { PlanDiffViewer } from "../../components/diff/PlanDiffViewer";
import type { FieldChange } from "../../components/diff/PlanDiffViewer";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import {
  CONNECTION_BLOCK,
  DATA_SOURCE_TYPES,
  dataSourceSchema,
  dataSourceUiSchema,
} from "../../schemas/kinds";
import type { DataSourceType } from "../../schemas/kinds";

/** The form of one source: the metadata a manifest carries plus the block its type names. */
export interface DataSourceForm {
  name?: string;
  title?: Record<string, string>;
  [block: string]: unknown;
}

const PLURAL = "datasources";

/** The manifest a form produces (MF-01, MF-35). */
export function toEnvelope(project: string, type: DataSourceType, form: DataSourceForm) {
  const { name, title, ...connection } = form;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: {
      name: name ?? "",
      namespace: project,
      ...(title && Object.keys(title).length > 0 ? { title } : {}),
    },
    spec: { type, ...prune(connection) },
  };
}

/** The form one manifest fills, so editing starts from what is in Git rather than from blank. */
export function toForm(source: Manifest): DataSourceForm {
  const connection = { ...source.spec };
  // The type is the selector's, not the form's: it decides which block the form even has.
  delete connection.type;
  return {
    name: source.metadata.name,
    ...(source.metadata.title ? { title: source.metadata.title } : {}),
    ...connection,
  };
}

/** Where a source connects to, in one line of the table. */
export function endpointOf(spec: Record<string, unknown>): string {
  for (const block of Object.values(CONNECTION_BLOCK)) {
    const connection = spec[block] as { url?: string; urls?: string[] } | undefined;
    if (connection?.url) {
      return connection.url;
    }
    if (connection?.urls?.length) {
      return connection.urls.join(", ");
    }
  }
  return "";
}

/**
 * The secret names this project already references, so the picker offers what exists instead
 * of inviting a typo (CC-06).
 *
 * The Portal never lists the secret store itself: a name is not a value, and a name nobody uses
 * yet is typed in full and read by an approver with the merge request.
 */
export function knownSecretNames(manifests: Manifest[]): string[] {
  const names = new Set<string>();
  const walk = (value: unknown): void => {
    if (Array.isArray(value)) {
      value.forEach(walk);
      return;
    }
    if (!value || typeof value !== "object") {
      return;
    }
    for (const [key, member] of Object.entries(value)) {
      if (key.endsWith("Ref") || key === "secretRefs") {
        for (const entry of Array.isArray(member) ? member : [member]) {
          const named = (entry ?? {}) as { name?: unknown };
          if (typeof named.name === "string") {
            names.add(named.name);
          }
        }
      }
      walk(member);
    }
  };
  manifests.forEach((manifest) => {
    walk(manifest.spec);
  });
  return [...names].sort();
}

/** The type a stored manifest declares, or the first one for a manifest that lost it. */
function typeOf(spec: Record<string, unknown>): DataSourceType {
  const declared = spec["type"];
  return DATA_SOURCE_TYPES.find((candidate) => candidate === declared) ?? DATA_SOURCE_TYPES[0];
}

/** DataSources of one project: what the city reads from, and with which credential (MF-35). */
export function DataSourcesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const [type, setType] = useState<DataSourceType>(DATA_SOURCE_TYPES[0]);
  const [editing, setEditing] = useState<Manifest | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [draft, setDraft] = useState<DataSourceForm | undefined>(undefined);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [plan, setPlan] = useState<FieldChange[] | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, PLURAL),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: PLURAL } },
        }),
      ),
  });

  const sources = useMemo(() => asManifests(list.data?.items ?? []), [list.data]);
  const secrets = useMemo(() => knownSecretNames(sources), [sources]);

  /** One request for both buttons: a dry run differs from a proposal only in the query. */
  const write = async (form: DataSourceForm, dry: boolean) => {
    const body = toEnvelope(project, type, form) as never;
    const query = dry ? { dryRun: "All" } : undefined;
    const name = editing?.metadata.name;
    return unwrap(
      name === undefined
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: PLURAL }, query },
            body,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural: PLURAL, name }, query },
            body,
          }),
    );
  };

  const reportError = (err: unknown) => {
    setFormError(
      err instanceof ApiError
        ? (err.problem?.detail ?? err.message)
        : err instanceof Error
          ? err.message
          : t("app.error.generic"),
    );
  };

  /** MF-13: the plan is read before the change is proposed, not after it is open. */
  const check = useMutation({
    mutationFn: async (form: DataSourceForm) => {
      setFormError(null);
      return write(form, true);
    },
    onSuccess: (result) => {
      setPlan((result as { plan?: { fields?: FieldChange[] } }).plan?.fields ?? []);
    },
    onError: (err) => {
      setPlan(null);
      reportError(err);
    },
  });

  const propose = useMutation({
    mutationFn: async (form: DataSourceForm) => {
      setFormError(null);
      return write(form, false);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      closeDialog();
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, PLURAL) });
    },
    onError: reportError,
  });

  function closeDialog() {
    setDialogOpen(false);
    setEditing(null);
    setDraft(undefined);
    setPlan(null);
  }

  function openCreate() {
    setEditing(null);
    setDraft(undefined);
    setPlan(null);
    setFormError(null);
    setDialogOpen(true);
  }

  function openEdit(source: Manifest) {
    setEditing(source);
    setType(typeOf(source.spec));
    setDraft(toForm(source));
    setPlan(null);
    setFormError(null);
    setDialogOpen(true);
  }

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

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-bold">{t("datasources.title")}</h1>
          <p className="mt-1 max-w-2xl text-sm text-surface-fg/70">{t("datasources.lead")}</p>
        </div>
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex flex-col text-sm">
            <span className="mb-1 font-medium">{t("datasources.field.type")}</span>
            <select
              value={type}
              onChange={(event) => {
                // The block a form offers is the one the type names, so a new type starts a
                // new draft rather than carrying the previous connection into it.
                setType(event.target.value as DataSourceType);
                setDraft(undefined);
                setPlan(null);
              }}
              className="rounded border border-border bg-surface px-3 py-1.5 text-sm text-surface-fg focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {DATA_SOURCE_TYPES.map((option) => (
                <option key={option} value={option}>
                  {t(`datasources.type.${option}`)}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            onClick={openCreate}
            className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
          >
            {t("datasources.add")}
          </button>
        </div>
      </div>

      {change ? <ChangeNotice change={change} project={project} /> : null}

      {sources.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("datasources.empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left text-sm">
            <caption className="sr-only">{t("datasources.title")}</caption>
            <thead>
              <tr className="border-b border-border bg-surface-subtle">
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("datasources.field.name")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("datasources.field.type")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("datasources.field.endpoint")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("datasources.field.secrets")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  <span className="sr-only">{t("datasources.edit")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {sources.map((source) => {
                const used = knownSecretNames([source]);
                return (
                  <tr key={source.metadata.name} className="border-b border-border last:border-0">
                    <td className="px-4 py-2">
                      <span className="font-mono">{source.metadata.name}</span>
                      {/* A source without a title is its name; printing it twice says nothing. */}
                      {source.metadata.title ? (
                        <span className="block text-xs text-surface-fg/70">
                          {localized(source.metadata.title, locale, source.metadata.name)}
                        </span>
                      ) : null}
                    </td>
                    <td className="px-4 py-2">{t(`datasources.type.${typeOf(source.spec)}`)}</td>
                    <td className="break-all px-4 py-2 font-mono text-xs">
                      {endpointOf(source.spec)}
                    </td>
                    {/* The names of the references, never a value: the store holds the rest. */}
                    <td className="px-4 py-2 text-xs">
                      {used.length > 0 ? used.join(", ") : t("datasources.noSecret")}
                    </td>
                    <td className="px-4 py-2 text-right">
                      <button
                        type="button"
                        onClick={() => openEdit(source)}
                        className="rounded border border-border px-3 py-1 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                      >
                        {t("datasources.edit")}
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <ResourceFormDialog<DataSourceForm>
        open={dialogOpen}
        onOpenChange={(open) => {
          if (!open) {
            closeDialog();
          }
        }}
        title={editing ? t("datasources.dialog.edit") : t("datasources.dialog.create")}
        description={t(`datasources.dialog.${type}`)}
        schema={dataSourceSchema(t, type, secrets)}
        // A rename is a new manifest at a new path, so the name is fixed once it exists.
        uiSchema={editing ? { ...dataSourceUiSchema, name: { "ui:readonly": true } } : dataSourceUiSchema}
        formData={draft}
        submitLabel={t("datasources.propose")}
        disabled={propose.isPending}
        error={formError}
        onChange={(data) => {
          setDraft(data);
          setPlan(null);
        }}
        onSubmit={(data) => propose.mutate(data)}
      >
        <div className="space-y-2">
          <p className="text-sm text-surface-fg/70">{t("datasources.secretHint")}</p>
          <button
            type="button"
            disabled={!draft?.name || check.isPending}
            onClick={() => {
              if (draft) {
                check.mutate(draft);
              }
            }}
            className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
          >
            {t("datasources.check")}
          </button>
          {plan ? (
            <div>
              <h2 className="text-sm font-medium">{t("datasources.plan")}</h2>
              <PlanDiffViewer fields={plan} />
            </div>
          ) : null}
        </div>
      </ResourceFormDialog>
    </div>
  );
}
