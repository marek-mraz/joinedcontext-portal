import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { prune } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { useProposal } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { SYNC_ORIGINS, syncSourceSchema } from "../../schemas/kinds";
import type { SyncOriginKind } from "../../schemas/kinds";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import type { components } from "../../api/schema";
import { Alert, Button, EmptyState, PageHeader, Select } from "../../components/ui";

export function syncStatusKey(project: string, name: string) {
  return ["projects", project, "syncsources", name, "status"] as const;
}

/** What the manifest says about where a source reads from (MF-27). */
interface SyncSourceSpec {
  source?: {
    git?: { url?: string; ref?: string; path?: string };
    bundle?: { url?: string };
    platformApi?: { baseUrl?: string; project?: string };
  };
  schedule?: { interval?: string; webhook?: boolean };
  mode?: string;
}

/** What the Add source dialog holds: a name, one origin, and how the run behaves (MF-27, MF-28). */
interface SyncSourceForm {
  name: string;
  title?: string;
  git?: { url: string; ref: string; path?: string; secretRef?: { name: string; key: string } };
  bundle?: { url: string; secretRef?: { name: string; key: string } };
  platformApi?: { baseUrl: string; project: string; secretRef?: { name: string; key: string } };
  interval: string;
  mode: string;
  conflictPolicy: string;
  prune?: boolean;
  autoMerge?: boolean;
}

export function syncSourceToManifest(project: string, form: SyncSourceForm) {
  const { name, title, git, bundle, platformApi, interval, mode, conflictPolicy, ...rest } = form;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "SyncSource",
    metadata: { name, namespace: project, ...(title?.trim() ? { title } : {}) },
    spec: prune({
      source: { git, bundle, platformApi },
      schedule: { interval },
      mode,
      conflictPolicy,
      ...rest,
    }),
  };
}

/** The form a manifest read back as YAML fills, so the dialog's two views stay one resource. */
export function syncSourceFromManifest(document: unknown): SyncSourceForm {
  const manifest = (document ?? {}) as { metadata?: { name?: string }; spec?: SyncSourceSpec };
  const spec = manifest.spec ?? {};
  return {
    name: manifest.metadata?.name ?? "",
    git: spec.source?.git as SyncSourceForm["git"],
    bundle: spec.source?.bundle as SyncSourceForm["bundle"],
    platformApi: spec.source?.platformApi as SyncSourceForm["platformApi"],
    interval: spec.schedule?.interval ?? "6h",
    mode: spec.mode ?? "mirror",
    conflictPolicy: (spec as { conflictPolicy?: string }).conflictPolicy ?? "fail",
  };
}

/**
 * The sync sources of one project: where each reads from, where it stands, and the three
 * things an operator can do to it (MF-27…MF-32, Architecture/06 section 6).
 *
 * The three buttons are three different kinds of act, and the page says so rather than
 * lining them up as equals. **Sync now** runs the loop once. **Pause** switches it off and
 * changes nothing in Git. **Detach** removes the source, which is a change to the repository
 * and therefore a merge request somebody has to approve — the resources the source brought in
 * stay where they are.
 */
export function SyncSourcesPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const list = useQuery({
    queryKey: queryKeys.list(project, "syncsources"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "syncsources" } },
        }),
      ),
  });

  const items = list.data?.items ?? [];

  // A source is added here or nowhere: MF-27 has no other door into the Portal (T-0790).
  const [origin, setOrigin] = useState<SyncOriginKind>("git");
  const [adding, setAdding] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const proposal = useProposal(project, "syncsources", (proposed) => {
    setChange(proposed);
    setAdding(false);
  });

  return (
    <section aria-label={t("syncSources.title")} className="space-y-6">
      <PageHeader
        title={t("syncSources.title")}
        description={t("syncSources.intro")}
        actions={
          <div className="flex items-end gap-2">
            <label className="flex flex-col gap-1 text-sm">
              {t("syncSources.origin")}
              <Select
                value={origin}
                onChange={(event) => setOrigin(event.target.value as SyncOriginKind)}
              >
                {SYNC_ORIGINS.map((kind) => (
                  <option key={kind} value={kind}>
                    {t(`syncSources.originKind.${kind}`)}
                  </option>
                ))}
              </Select>
            </label>
            <Button size="sm" variant="primary" onClick={() => setAdding(true)}>
              {t("syncSources.add")}
            </Button>
          </div>
        }
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <ResourceFormDialog<SyncSourceForm>
        kind="SyncSource"
        open={adding}
        onOpenChange={(open) => {
          if (!open) {
            setAdding(false);
            proposal.reset();
          }
        }}
        project={project}
        title={t("syncSources.dialog.title")}
        description={t("syncSources.dialog.description")}
        schema={syncSourceSchema(t, origin)}
        formData={{ name: "", interval: "6h", mode: "mirror", conflictPolicy: "fail" }}
        submitLabel={t("syncSources.propose")}
        submitting={proposal.mutation.isPending}
        error={proposal.error}
        source={{
          toManifest: (form) => syncSourceToManifest(project, form),
          fromManifest: syncSourceFromManifest,
        }}
        onSubmit={(form) =>
          proposal.mutation.mutate({ body: syncSourceToManifest(project, form), create: true })
        }
      />

      {list.isPending ? <p role="status">{t("app.loading")}</p> : null}
      {list.isError ? (
        <Alert role="alert" tone="danger">
          {t("app.error.generic")}
        </Alert>
      ) : null}

      {!list.isPending && !list.isError && items.length === 0 ? (
        <EmptyState title={t("syncSources.empty")} />
      ) : null}

      <ul className="space-y-4">
        {items.map((item) => (
          <li key={item.metadata.name}>
            <SyncSourceCard
              project={project}
              name={item.metadata.name}
              spec={item.spec as SyncSourceSpec | undefined}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}

function SyncSourceCard({
  project,
  name,
  spec,
}: {
  project: string;
  name: string;
  spec: SyncSourceSpec | undefined;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [detached, setDetached] = useState<string | null>(null);

  const status = useQuery({
    queryKey: syncStatusKey(project, name),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/syncsources/{name}/status", {
          params: { path: { project, name } },
        }),
      ),
  });

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: syncStatusKey(project, name) });
  };

  const syncNow = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/syncsources/{name}/sync", {
          params: { path: { project, name } },
        }),
      ),
    onSuccess: refresh,
  });

  const pause = useMutation({
    mutationFn: async (paused: boolean) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/syncsources/{name}/pause", {
          params: { path: { project, name } },
          body: { paused },
        }),
      ),
    onSuccess: refresh,
  });

  const detach = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/syncsources/{name}/detach", {
          params: { path: { project, name } },
        }),
      ),
    onSuccess: (result) => {
      const url = (result as { mergeRequest?: string } | undefined)?.mergeRequest;
      setDetached(url ?? null);
      refresh();
    },
  });

  const busy = syncNow.isPending || pause.isPending || detach.isPending;
  const failure = [syncNow.error, pause.error, detach.error].find(Boolean);
  const paused = status.data?.paused ?? false;

  return (
    <article className="rounded border border-border p-4">
      <header className="flex flex-wrap items-baseline gap-3">
        <h3 className="font-mono text-sm font-bold">{name}</h3>
        <LifecycleBadge kind="phase" value={status.data?.phase} />
        <span className="text-xs">{scheduleOf(spec, t)}</span>
        <span className="ml-auto flex items-center gap-1.5">
          <EditResourceAction target={{ project, kind: "SyncSource", plural: "syncsources", name }} />
          <DeleteResourceAction target={{ project, kind: "SyncSource", plural: "syncsources", name }} />
        </span>
      </header>

      <dl className="mt-3 grid gap-x-6 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]">
        <dt className="font-medium">{t("syncSources.origin")}</dt>
        <dd className="break-all font-mono text-xs">{originOf(spec) ?? "—"}</dd>

        <dt className="font-medium">{t("syncSources.revision")}</dt>
        <dd className="font-mono text-xs">
          {status.data?.observedRevision ? status.data.observedRevision.slice(0, 12) : "—"}
        </dd>

        {status.data?.mergeRequest ? (
          <>
            <dt className="font-medium">{t("syncSources.mergeRequest")}</dt>
            <dd>
              <a
                href={status.data.mergeRequest}
                className="underline"
                rel="noreferrer"
                target="_blank"
              >
                {t("syncSources.review")}
              </a>
            </dd>
          </>
        ) : null}

        {status.data?.lastError ? (
          <>
            <dt className="font-medium">{t("syncSources.lastError")}</dt>
            <dd className="text-danger">{status.data.lastError}</dd>
          </>
        ) : null}
      </dl>

      {status.data && !status.data.durable ? (
        <p className="mt-3 text-xs">{t("syncSources.notDurable")}</p>
      ) : null}

      <RunReport report={syncNow.data} />

      {detached ? (
        <p role="status" className="mt-3 text-sm">
          {t("syncSources.detached")}{" "}
          <a href={detached} className="underline" rel="noreferrer" target="_blank">
            {t("syncSources.review")}
          </a>
        </p>
      ) : null}

      {failure ? (
        <Alert role="alert" tone="danger" className="mt-3">
          {failure instanceof ApiError ? failure.message : t("app.error.generic")}
        </Alert>
      ) : null}

      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          size="sm"
          disabled={busy || paused}
          onClick={() => syncNow.mutate()}
        >
          {t("syncSources.syncNow")}
        </Button>
        <Button
          size="sm"
          disabled={busy}
          onClick={() => pause.mutate(!paused)}
        >
          {paused ? t("syncSources.resume") : t("syncSources.pause")}
        </Button>
        <Button
          variant="danger"
          size="sm"
          disabled={busy}
          onClick={() => {
            if (window.confirm(t("syncSources.detachConfirm", { name }))) {
              detach.mutate();
            }
          }}
        >
          {t("syncSources.detach")}
        </Button>
      </div>
    </article>
  );
}

/** What a forced run did, when one has been asked for. */
function RunReport({ report }: { report: components["schemas"]["SyncRunReport"] | undefined }) {
  const { t } = useTranslation();
  if (!report) {
    return null;
  }
  return (
    <div className="mt-3 space-y-1 text-sm">
      {report.proposed.length > 0 ? (
        <p role="status">{t("syncSources.proposed", { count: report.proposed.length })}</p>
      ) : report.flags.length === 0 ? (
        <p role="status">{t("syncSources.nothingToDo")}</p>
      ) : null}
      {report.flags.map((flag) => (
        <Alert key={flag} role="alert" tone="danger" className="mt-1">
          {flag}
        </Alert>
      ))}
    </div>
  );
}

/** The origin in one line, whichever of the three a source names. */
function originOf(spec: SyncSourceSpec | undefined): string | undefined {
  const source = spec?.source;
  if (source?.git?.url) {
    const subtree = source.git.path ? ` (${source.git.path})` : "";
    return `${source.git.url}#${source.git.ref ?? "HEAD"}${subtree}`;
  }
  if (source?.bundle?.url) {
    return source.bundle.url;
  }
  if (source?.platformApi?.baseUrl) {
    return `${source.platformApi.baseUrl}/${source.platformApi.project ?? ""}`;
  }
  return undefined;
}

/** How often it runs, in the words the manifest uses (MF-28). */
function scheduleOf(
  spec: SyncSourceSpec | undefined,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (spec?.schedule?.webhook) {
    return t("syncSources.byWebhook");
  }
  if (spec?.schedule?.interval) {
    return t("syncSources.everyInterval", { interval: spec.schedule.interval });
  }
  return "";
}
