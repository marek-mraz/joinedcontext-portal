import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { Icon } from "../../components/ui/icons";
import { requestOpen } from "../../assistant/state";
import { AgentRunPage } from "./AgentRunPage";
import { appDisplayName, useEndpointTitles } from "./appTitle";
import { runInUrl, setRunInUrl } from "./useAgentRun";
import { PageHeader } from "../../components/ui/PageHeader";

interface DataNeed {
  contextSpaceRef?: string | { name?: string };
  types?: string[];
  attrs?: string[];
}

export interface AppSpec {
  /** `static` or `full-stack`; `spec.kind` in the manifest (AP-01). */
  kind?: string;
  visibility?: string;
  /** `draft` → `preview` → `published` → `retired` (AP-18). */
  lifecycle?: string;
  /** Whether the Portal may frame the app at all (AP-12). */
  embeddable?: boolean;
  dataNeeds?: DataNeed[];
}

export function appSpec(app: Manifest): AppSpec {
  return app.spec as AppSpec;
}

export function draftState(status: string): "building" | "needsYou" | "failed" | "readyToPublish" | null {
  switch (status) {
    case "queued":
    case "starting":
    case "building":
    case "testing":
    case "previewing":
      return "building";
    case "interviewing":
      return "needsYou";
    case "awaiting_approval":
    case "awaitingApproval":
      return "readyToPublish";
    case "failed":
    case "cancelled":
    case "expired":
      return "failed";
    case "published":
      return null;
    default:
      return null;
  }
}

interface CatalogRun {
  id: string;
  appName: string;
  /** What the application is called; absent from older runs, which show the name as words. */
  title?: string;
  endpointName?: string;
  status: string;
  prompt?: string;
  error?: string;
  createdAt: string;
}

/** The space a data need names, whichever of the two `Ref` spellings the manifest used. */
function spaceOf(need: DataNeed): string {
  return refName(need.contextSpaceRef);
}

/**
 * The preview URL of Architecture/16 §4: the app under the platform host, with the commit it
 * was built from, so a reviewer is never looking at a cached older build.
 */
export function previewUrl(app: Manifest): string {
  const revision = app.status?.observedRevision;
  const at = revision ? `?preview=${encodeURIComponent(revision)}` : "";
  return `/apps/${encodeURIComponent(app.metadata.name)}/${at}`;
}

/**
 * The sandboxed preview of AP-19.
 *
 * `sandbox` deliberately does NOT carry `allow-same-origin`. A static app is served from
 * `https://{host}/apps/{name}/` (AP-14), which is the Portal's own origin, and the pair
 * `allow-scripts allow-same-origin` on a same-origin frame is not a sandbox at all: the
 * framed document could reach into the Portal, read the deliberately readable `jc_csrf`
 * cookie and issue writes as the signed-in reviewer, whose session cookie the browser would
 * attach. With `allow-scripts` alone the frame runs its code in an opaque origin, which is
 * what makes previewing somebody else's generated app safe to do while signed in.
 *
 * The server half is already in place: the static host sends `frame-ancestors 'none'` and
 * `X-Frame-Options: DENY` for an app whose manifest does not say `embeddable` (AP-12), so an
 * app that may not be framed shows its reason here rather than an empty rectangle.
 */
export function AppPreview({ app, onClose }: { app: Manifest; onClose: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const spec = appSpec(app);
  const title = localized(app.metadata.title, i18n.language, app.metadata.name);

  return (
    <div className="space-y-3">
      <button
        type="button"
        onClick={onClose}
        className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
      >
        {t("apps.back")}
      </button>

      <PageHeader title={t("apps.preview.title", { name: title })} description={t("apps.preview.sandboxSpace")} />

      {spec.embeddable ? (
        <iframe
          title={t("apps.preview.title", { name: title })}
          src={previewUrl(app)}
          sandbox="allow-scripts"
          referrerPolicy="no-referrer"
          className="h-[32rem] w-full rounded border border-border bg-surface"
        />
      ) : (
        <p role="alert" className="text-danger">
          {t("apps.preview.notEmbeddable")}
        </p>
      )}
    </div>
  );
}

/**
 * The apps catalog (AP-18…AP-20, AP-24).
 *
 * Publishing is a repository change like every other write in the Portal: the manifest's
 * lifecycle flips to `published` and the answer is a merge request, never a live switch. The
 * lane the change lands in follows the app's `visibility`, which is why the confirmation says
 * what is about to become reachable and to whom.
 */
export function AppsCatalog({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [previewing, setPreviewing] = useState<Manifest | null>(null);
  // A run stays where it is: the address names it, so a closed tab reopens the same build.
  const [runId, setRunId] = useState<string | null>(() => runInUrl());
  const [confirming, setConfirming] = useState<Manifest | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "apps"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "apps" } },
        }),
      ),
  });

  const runs = useQuery({
    queryKey: [...queryKeys.list(project, "apps"), "runs"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project }, query: { limit: 20 } },
        }),
      ),
    refetchInterval: runId === null ? 15000 : false,
  });
  const endpointTitles = useEndpointTitles(project);

  const publish = useMutation({
    mutationFn: async (app: Manifest) => {
      setError(null);
      // `status` is the API's own computation (MF-04); a write never sends it back.
      const body = {
        apiVersion: app.apiVersion,
        kind: app.kind,
        metadata: app.metadata,
        spec: { ...appSpec(app), lifecycle: "published" },
      } as never;
      return unwrap(
        await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "apps", name: app.metadata.name } },
          body,
        }),
      );
    },
    onSuccess: (result) => {
      setConfirming(null);
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "apps") });
    },
    onError: (err) => {
      setConfirming(null);
      setError(
        err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"),
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

  if (previewing) {
    return (
      <AppPreview
        app={previewing}
        onClose={() => {
          setPreviewing(null);
        }}
      />
    );
  }

  if (runId !== null) {
    return (
      <AgentRunPage
        project={project}
        runId={runId}
        onClose={() => {
          setRunInUrl(null);
          setRunId(null);
          void runs.refetch();
        }}
      />
    );
  }

  const apps = asManifests(list.data?.items ?? []);
  const publishedNames = new Set(apps.map((app) => app.metadata.name));

  const runItems = (runs.data?.items ?? []) as unknown as CatalogRun[];
  const draftRuns: CatalogRun[] = [];
  const seenDraftApps = new Set<string>();
  for (const r of runItems) {
    if (r.appName && !publishedNames.has(r.appName) && !seenDraftApps.has(r.appName)) {
      seenDraftApps.add(r.appName);
      const state = draftState(r.status);
      // An expired, cancelled or failed build is history, not an application: it stays in the
      // assistant's past work, not in this grid.
      if (state !== null && state !== "failed") {
        draftRuns.push(r);
      }
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <PageHeader title={t("apps.title")} description={t("apps.subtitle")} />
        <PermissionGuard project={project} kind="App" verb="propose">
          <button
            type="button"
            // The builder lives in the assistant: the same panel on every page (UI-45).
            onClick={() => {
              requestOpen("build");
            }}
            className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50"
          >
            {t("apps.newAction")}
          </button>
        </PermissionGuard>
      </div>

      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <p role="alert" className="text-danger">
          {error}
        </p>
      )}

      {apps.length === 0 && draftRuns.length === 0 && <p>{t("apps.empty")}</p>}

      <ul className="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-3">
        {apps.map((app) => {
          const spec = appSpec(app);
          const title =
            localized(app.metadata.title, i18n.language, "") || appDisplayName({ appName: app.metadata.name });
          const needs = spec.dataNeeds ?? [];
          return (
            <li
              key={app.metadata.name}
              title={localized(app.metadata.description, i18n.language, "") || undefined}
              className="flex flex-col items-center gap-2 rounded-xl border border-border bg-surface p-4 text-center hover:bg-surface-subtle"
            >
              <AppIcon />
              <h2 className="line-clamp-2 text-sm font-semibold">{title}</h2>
              <LifecycleBadge kind="appLifecycle" value={spec.lifecycle ?? "draft"} />
              {spec.visibility ? (
                <p className="text-xs text-muted">
                  {t("apps.visibility", { visibility: spec.visibility })}
                </p>
              ) : null}
              {needs.length > 0 && (
                <p className="line-clamp-2 text-xs text-muted">
                  {t("apps.dataNeeds", {
                    spaces: [...new Set(needs.map(spaceOf).filter(Boolean))].join(", "),
                    types: [...new Set(needs.flatMap((need) => need.types ?? []))].join(", "),
                  })}
                </p>
              )}

              <div className="mt-auto flex flex-wrap justify-center gap-2">
                {/* Only a preview is framed here: a published app is reached by its own
                    audience, and a draft has nothing deployed yet (AP-18, AP-19). */}
                {spec.lifecycle === "preview" && (
                  <button
                    type="button"
                    onClick={() => {
                      setPreviewing(app);
                    }}
                    className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                  >
                    {t("apps.previewAction")}
                  </button>
                )}
                {spec.lifecycle === "preview" && (
                  <button
                    type="button"
                    onClick={() => {
                      setConfirming(app);
                    }}
                    className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                  >
                    {t("apps.publishAction")}
                  </button>
                )}
                {/* AP-24: every iteration with the agent is a commit, and the prompt history
                    lives with the source. The forge shows both; the Portal does not keep a
                    second copy of a Git log. */}
                {app.status?.sourceUrl ? (
                  <a
                    href={app.status.sourceUrl}
                    target="_blank"
                    rel="noreferrer noopener"
                    className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                  >
                    {t("apps.history")}
                  </a>
                ) : null}
                <EditResourceAction
                  target={{ project, kind: "App", plural: "apps", name: app.metadata.name, label: title }}
                />
                <DeleteResourceAction
                  target={{ project, kind: "App", plural: "apps", name: app.metadata.name, label: title }}
                />
              </div>
            </li>
          );
        })}
        {draftRuns.map((draft) => {
          const state = draftState(draft.status);
          if (!state) return null;
          return (
            <li
              key={draft.id}
              title={draft.prompt || undefined}
              className="flex flex-col items-center gap-2 rounded-xl border border-border bg-surface p-4 text-center hover:bg-surface-subtle"
            >
              <AppIcon />
              <h2 className="line-clamp-2 text-sm font-semibold">
                {appDisplayName({
                  title: draft.title,
                  appName: draft.appName,
                  endpointTitle: draft.endpointName ? endpointTitles.get(draft.endpointName) : undefined,
                })}
              </h2>
              <span className="rounded bg-surface-subtle px-2 py-0.5 text-xs font-medium text-fg-muted">
                {t(`apps.drafts.state.${state}`)}
              </span>
              <div className="mt-auto flex flex-wrap justify-center gap-2">
                <Link
                  to="/projects/$project/apps/$name"
                  params={{ project, name: draft.appName }}
                  className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                >
                  {t("apps.drafts.open")}
                </Link>
              </div>
            </li>
          );
        })}
      </ul>

      {confirming && (
        <PublishDialog
          app={confirming}
          pending={publish.isPending}
          onCancel={() => {
            setConfirming(null);
          }}
          onConfirm={() => {
            publish.mutate(confirming);
          }}
        />
      )}
    </div>
  );
}

/** The tile's icon: every application gets the same mark until a manifest carries its own. */
function AppIcon(): JSX.Element {
  return (
    <span className="flex size-14 items-center justify-center rounded-2xl bg-primary-soft text-primary-soft-fg">
      <Icon name="apps" className="size-7" />
    </span>
  );
}

/**
 * The publication confirmation (AP-20). It says what publishing does rather than asking "are
 * you sure": the app leaves its sandbox space, binds to the real one, and becomes reachable
 * by the audience its `visibility` names.
 */
function PublishDialog({
  app,
  pending,
  onCancel,
  onConfirm,
}: {
  app: Manifest;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const spec = appSpec(app);
  const title = localized(app.metadata.title, i18n.language, app.metadata.name);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={t("apps.publish.title", { name: title })}
      className="rounded border border-border bg-surface-subtle p-4"
    >
      <h2 className="font-semibold">{t("apps.publish.title", { name: title })}</h2>
      <p className="mt-2 text-sm">
        {t("apps.publish.body", { visibility: spec.visibility ?? "project" })}
      </p>
      <p className="mt-1 text-sm text-muted">{t("apps.publish.hint")}</p>
      <div className="mt-3 flex gap-2">
        <button
          type="button"
          disabled={pending}
          onClick={onConfirm}
          className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {t("apps.publish.confirm")}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("apps.publish.cancel")}
        </button>
      </div>
    </div>
  );
}
