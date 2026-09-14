import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../../api/permissions";
import { takePrefill } from "../../assistant/state";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, prune } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import type { Verdict } from "../../api/drafts";
import { ChangeNotice } from "../../components/ChangeNotice";
import { PlanDiffViewer } from "../../components/diff/PlanDiffViewer";
import type { FieldChange } from "../../components/diff/PlanDiffViewer";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { stringify as stringifyYaml } from "yaml";
import {
  CONNECTION_BLOCK,
  DATA_SOURCE_TYPES,
  dataSourceSchema,
  dataSourceUiSchema,
  findEnvVars,
  isTypedDataSource,
  parseYamlStrings,
  runnerDataSourceSchema,
  yamlPathsOf,
  YamlFieldError,
} from "../../schemas/kinds";
import type { CatalogInput, DataSourceType, TypedDataSourceType } from "../../schemas/kinds";
import { useBentoInputs } from "./RunnerInputForm";
import { SecretRefContext } from "../../components/forms/widgets/SecretRef";
import type { SecretRefValue } from "../../components/forms/widgets/SecretRef";

/** The form of one source: the metadata a manifest carries plus the block its type names. */
export interface DataSourceForm {
  name?: string;
  title?: Record<string, string>;
  [block: string]: unknown;
}

const PLURAL = "datasources";

/** One fetch of an `http` source on the runner, beside the dry run's plan (MF-39). */
interface Probe {
  records?: number;
  bytes?: number;
  sample?: unknown;
  skipped?: string;
}

/** The manifest a form produces (MF-01, MF-35). */
export interface DataSourceEnvelope {
  apiVersion: string;
  kind: "DataSource";
  metadata: { name: string; namespace: string; title?: unknown };
  spec: Record<string, unknown>;
}

export function toEnvelope(
  project: string,
  type: DataSourceType,
  form: DataSourceForm,
  inputDef?: CatalogInput
): DataSourceEnvelope {
  if (isTypedDataSource(type)) {
    const { name, title, ...connection } = form;
    delete connection.secrets;
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

  const { name, title, secrets: formSecrets = [], ...connection } = form;
  const rawInput =
    connection.input && typeof connection.input === "object" && !Array.isArray(connection.input)
      ? { ...(connection.input as Record<string, unknown>), ...connection }
      : { ...connection };
  delete rawInput.input;

  const parsedInput = parseYamlStrings(rawInput, yamlPathsOf(inputDef));
  const cleanedInput = prune(parsedInput) as Record<string, unknown>;

  const usedEnvVars = new Set<string>();
  findEnvVars(cleanedInput, usedEnvVars);

  const secretsList = Array.isArray(formSecrets) ? formSecrets : [];
  const dedupedSecrets: Array<{ name: string; key?: string; envVar: string }> = [];
  const seenEnvVars = new Set<string>();
  for (const s of secretsList as SecretRefValue[]) {
    if (s && typeof s === "object" && typeof s.envVar === "string") {
      if (usedEnvVars.has(s.envVar) && !seenEnvVars.has(s.envVar)) {
        seenEnvVars.add(s.envVar);
        dedupedSecrets.push({ name: s.name, key: s.key, envVar: s.envVar });
      }
    }
  }

  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: {
      name: name ?? "",
      namespace: project,
      ...(title && Object.keys(title).length > 0 ? { title } : {}),
    },
    spec: {
      type,
      input: cleanedInput,
      ...(dedupedSecrets.length > 0 ? { secrets: dedupedSecrets } : {}),
    },
  };
}

/** The form one manifest fills, so editing starts from what is in Git rather than from blank. */
export function toForm(source: Manifest): DataSourceForm {
  const spec = source.spec || {};
  const type = typeOf(spec);
  if (!isTypedDataSource(type)) {
    const input =
      spec.input && typeof spec.input === "object" && !Array.isArray(spec.input)
        ? (spec.input as Record<string, unknown>)
        : {};
    const flattened: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(input)) {
      if (Array.isArray(v) && v.length > 0 && typeof v[0] === "object" && v[0] !== null) {
        flattened[k] = stringifyYaml(v).trim();
      } else {
        flattened[k] = v;
      }
    }
    return {
      name: source.metadata.name,
      ...(source.metadata.title ? { title: source.metadata.title } : {}),
      ...flattened,
      ...(Array.isArray(spec.secrets) ? { secrets: spec.secrets } : {}),
    };
  }

  const connection = { ...spec };
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
  if (spec.input && typeof spec.input === "object") {
    const input = spec.input as Record<string, unknown>;
    const candidates = [input.url, input.urls, input.addresses, input.paths, input.dsn];
    for (const c of candidates) {
      if (typeof c === "string" && c.trim()) {
        return c.trim();
      }
      if (Array.isArray(c) && c.length > 0) {
        return c.join(", ");
      }
    }
  }
  if (typeof spec.type === "string" && spec.type.trim()) {
    return spec.type.trim();
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
      if (key.endsWith("Ref") || key === "secretRefs" || key === "secrets") {
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
export function typeOf(spec: Record<string, unknown>): DataSourceType {
  const declared = spec["type"];
  if (typeof declared === "string" && declared.trim()) {
    return declared.trim();
  }
  return DATA_SOURCE_TYPES[0];
}

const RUNNER_GROUPS = [
  "brokers",
  "files",
  "databases",
  "http",
  "streams",
  "queues",
  "utility",
] as const;

/** DataSources of one project: what the city reads from, and with which credential (MF-35). */
export function DataSourcesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const mayPropose = usePermissions(project).can("DataSource", "propose");
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  // The assistant may have sent the person here with a form in hand (UI-45, AG-61): taken
  // once, before the first render, so the dialog is open and filled from the start.
  const [urlDraftName, setUrlDraftName] = useState(() => {
    if (typeof window === "undefined") return undefined;
    return new URLSearchParams(window.location.search).get("draft") ?? undefined;
  });

  const [initial] = useState(() => {
    const taken = takePrefill(window.location.pathname) as (DataSourceForm & { type?: unknown }) | null;
    if (!taken) {
      return undefined;
    }
    const { type: prefillType, ...form } = taken;
    return { type: DATA_SOURCE_TYPES.find((candidate) => candidate === prefillType) ?? DATA_SOURCE_TYPES[0], form };
  });
  const [type, setType] = useState<DataSourceType>(initial?.type ?? DATA_SOURCE_TYPES[0]);
  const catalog = useBentoInputs();
  const [editing, setEditing] = useState<Manifest | null>(null);
  const [dialogOpen, setDialogOpen] = useState(initial !== undefined || urlDraftName !== undefined);
  const [draft, setDraft] = useState<DataSourceForm | undefined>(initial?.form);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [collectedSecrets, setCollectedSecrets] = useState<Record<string, SecretRefValue>>({});
  const [plan, setPlan] = useState<FieldChange[] | null>(null);
  const [probe, setProbe] = useState<Probe | null>(null);
  const [verdict, setVerdict] = useState<Verdict | null>(null);

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

  const runnerCatalogInput = catalog?.inputs.find((i) => i.name === type);
  const selectedSummary = runnerCatalogInput?.summary;

  const handleSecretRef = (envVar: string, ref: SecretRefValue) => {
    setCollectedSecrets((prev) => ({ ...prev, [envVar]: ref }));
  };

  const validateForm = (form: DataSourceForm): boolean => {
    try {
      toEnvelope(project, type, form, runnerCatalogInput);
      return true;
    } catch (err) {
      if (!(err instanceof YamlFieldError)) throw err;
      setFormError(
        t("datasources.runner.yamlError", {
          field: err.field,
          error: err.detail,
          defaultValue: err.message,
        })
      );
      return false;
    }
  };

  /** One request for both buttons: a dry run differs from a proposal only in the query. */
  const write = async (form: DataSourceForm, dry: boolean, draftRef?: { kind: string; name: string }) => {
    const envelope = toEnvelope(project, type, form, runnerCatalogInput);
    const body = (draftRef ? { ...envelope, draft: draftRef } : envelope) as never;
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
      const draftRef = form.name ? { kind: "DataSource", name: form.name } : undefined;
      return write(form, true, draftRef);
    },
    onSuccess: (result) => {
      const answer = result as { plan?: { fields?: FieldChange[] }; probe?: Probe; verdict?: Verdict };
      setPlan(answer.plan?.fields ?? []);
      setProbe(answer.probe ?? null);
      if (answer.verdict) {
        setVerdict(answer.verdict);
      }
    },
    onError: (err) => {
      setPlan(null);
      setProbe(null);
      reportError(err);
    },
  });

  const propose = useMutation({
    mutationFn: async ({ form, draft: draftRef }: { form: DataSourceForm; draft?: { kind: string; name: string } }) => {
      return write(form, false, draftRef);
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
    setProbe(null);
    setVerdict(null);
    setCollectedSecrets({});
    setUrlDraftName(undefined);
  }

  function openCreate() {
    setEditing(null);
    setDraft(undefined);
    setPlan(null);
    setProbe(null);
    setVerdict(null);
    setFormError(null);
    setCollectedSecrets({});
    setUrlDraftName(undefined);
    setDialogOpen(true);
  }

  function openEdit(source: Manifest) {
    setEditing(source);
    const storedType = typeOf(source.spec);
    setType(storedType);
    setDraft(toForm(source));
    setPlan(null);
    setProbe(null);
    setVerdict(null);
    setFormError(null);
    setUrlDraftName(source.metadata.name);
    if (Array.isArray(source.spec.secrets)) {
      const initial: Record<string, SecretRefValue> = {};
      for (const s of source.spec.secrets as SecretRefValue[]) {
        if (s.envVar) initial[s.envVar] = s;
      }
      setCollectedSecrets(initial);
    } else {
      setCollectedSecrets({});
    }
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
                setType(event.target.value as DataSourceType);
                setDraft(undefined);
                setPlan(null);
                setProbe(null);
                setCollectedSecrets({});
              }}
              className="rounded border border-border bg-surface px-3 py-1.5 text-sm text-surface-fg focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              <optgroup label={t("datasources.group.typed", { defaultValue: "Common connections" })}>
                {DATA_SOURCE_TYPES.map((option) => (
                  <option key={option} value={option}>
                    {t(`datasources.type.${option}`, { defaultValue: option })}
                  </option>
                ))}
              </optgroup>
              {catalog &&
                RUNNER_GROUPS.map((group) => {
                  const items = catalog.inputs.filter((i) => i.group === group);
                  if (items.length === 0) return null;
                  return (
                    <optgroup
                      key={group}
                      label={t(`datasources.group.${group}`, {
                        defaultValue: group.charAt(0).toUpperCase() + group.slice(1),
                      })}
                    >
                      {items.map((item) => (
                        <option key={item.name} value={item.name}>
                          {item.name}
                        </option>
                      ))}
                    </optgroup>
                  );
                })}
            </select>
          </label>
          {selectedSummary ? (
            <p data-testid="input-summary" className="max-w-xs text-xs text-surface-fg/70">
              {selectedSummary}
            </p>
          ) : null}
          <PermissionGuard project={project} kind="DataSource" verb="propose">
          <button
            type="button"
            onClick={openCreate}
            className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
          >
            {t("datasources.add")}
          </button>
          </PermissionGuard>
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
                    <td className="px-4 py-2">
                      {isTypedDataSource(typeOf(source.spec))
                        ? t(`datasources.type.${typeOf(source.spec)}`, { defaultValue: typeOf(source.spec) })
                        : typeOf(source.spec)}
                    </td>
                    <td className="break-all px-4 py-2 font-mono text-xs">
                      {endpointOf(source.spec)}
                    </td>
                    {/* The names of the references, never a value: the store holds the rest. */}
                    <td className="px-4 py-2 text-xs">
                      {used.length > 0 ? used.join(", ") : t("datasources.noSecret")}
                    </td>
                    <td className="px-4 py-2 text-right">
                      {mayPropose ? <button
                        type="button"
                        onClick={() => openEdit(source)}
                        className="rounded border border-border px-3 py-1 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                      >
                        {t("datasources.edit")}
                      </button> : null}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <SecretRefContext.Provider
        value={{
          knownSecretNames: secrets,
          onSecretRef: handleSecretRef,
          secrets: Object.values(collectedSecrets),
        }}
      >
        <ResourceFormDialog<DataSourceForm>
          open={dialogOpen}
          onOpenChange={(open) => {
            if (!open) {
              closeDialog();
            }
          }}
          project={project}
          draftKind="DataSource"
          draftName={editing?.metadata.name || urlDraftName || undefined}
          verdict={verdict}
          onVerdictChange={setVerdict}
          source={{
            toManifest: (form) =>
              toEnvelope(project, type, {
                ...form,
                secrets: Object.values(collectedSecrets),
              }),
            fromManifest: (manifest) => toForm(manifest as Manifest),
          }}
          title={editing ? t("datasources.dialog.edit") : t("datasources.dialog.create")}
          description={
            isTypedDataSource(type)
              ? t(`datasources.dialog.${type}`)
              : runnerCatalogInput?.summary ?? t("datasources.dialog.create")
          }
          schema={
            isTypedDataSource(type)
              ? dataSourceSchema(t, type as TypedDataSourceType, secrets)
              : runnerCatalogInput
                ? runnerDataSourceSchema(t, runnerCatalogInput).schema
                : {
                    type: "object",
                    required: ["name"],
                    properties: {
                      name: { type: "string", title: t("datasources.field.name") },
                    },
                  }
          }
          uiSchema={
            isTypedDataSource(type)
              ? editing
                ? { ...dataSourceUiSchema, name: { "ui:readonly": true } }
                : dataSourceUiSchema
              : runnerCatalogInput
                ? editing
                  ? { ...runnerDataSourceSchema(t, runnerCatalogInput).uiSchema, name: { "ui:readonly": true } }
                  : runnerDataSourceSchema(t, runnerCatalogInput).uiSchema
                : editing
                  ? { name: { "ui:readonly": true } }
                  : {}
          }
          formData={draft}
          submitLabel={t("datasources.propose")}
          disabled={propose.isPending}
          error={formError}
          onChange={(data) => {
            setDraft(data);
            setPlan(null);
            setProbe(null);
            setVerdict(null);
          }}
          onSubmit={(data, draftRef) => {
            setFormError(null);
            if (!validateForm(data)) {
              return;
            }
            propose.mutate({
              form: { ...data, secrets: Object.values(collectedSecrets) },
              draft: draftRef,
            });
          }}
        >
          <div className="space-y-2">
            <p className="text-sm text-surface-fg/70">{t("datasources.secretHint")}</p>
            <button
              type="button"
              disabled={!draft?.name || check.isPending}
              onClick={() => {
                if (draft) {
                  setFormError(null);
                  if (!validateForm(draft)) {
                    return;
                  }
                  check.mutate({
                    ...draft,
                    secrets: Object.values(collectedSecrets),
                  });
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
          {probe ? (
            <div data-testid="datasource-probe">
              <h2 className="text-sm font-medium">{t("datasources.probe.title")}</h2>
              {probe.skipped ? (
                <p role="status" className="text-sm text-surface-fg/70">
                  {t("datasources.probe.skipped", { reason: probe.skipped })}
                </p>
              ) : (
                <>
                  <p role="status" className="text-sm">
                    {t("datasources.probe.records", { records: probe.records ?? 0, bytes: probe.bytes ?? 0 })}
                  </p>
                  {probe.sample !== undefined ? (
                    <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono text-xs">
                      {JSON.stringify(probe.sample, null, 2)}
                    </pre>
                  ) : null}
                </>
              )}
            </div>
          ) : null}
        </div>
      </ResourceFormDialog>
      </SecretRefContext.Provider>
    </div>
  );
}
