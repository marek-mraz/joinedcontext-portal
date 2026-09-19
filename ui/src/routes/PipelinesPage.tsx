import { useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import { proposeChecked } from "../api/proposal";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ChangeNotice } from "../components/ChangeNotice";
import { ResourceList } from "../components/ResourceList";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { PipelineEditorDialog } from "../pages/pipelines/PipelineEditor";
import type { PipelineForm, toEnvelope } from "../pages/pipelines/PipelineEditor";
import { takeEditRequest, takePrefill } from "../assistant/state";
import { useProjectUsage } from "../components/ProjectQuota";
import { PermissionGuard } from "../components/ui/PermissionGuard";
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
  SourceLink,
} from "../components/ui";

interface SecretRef {
  name?: string;
  key?: string;
  envVar?: string;
}

interface PipelineSpec {
  class?: string;
  period?: string;
  schedule?: string;
  enabled?: boolean;
  secretRefs?: SecretRef[];
}

/** Bento's own duration notation, `250ms` / `45s` / `5m` / `1h` (Architecture/08 §1). */
function periodSeconds(period: string | undefined): number | undefined {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m|h)$/.exec(period?.trim() ?? "");
  if (!match) {
    return undefined;
  }
  const value = Number(match[1]);
  const factor = { ms: 0.001, s: 1, m: 60, h: 3600 }[match[2] as "ms" | "s" | "m" | "h"];
  return value * factor;
}

/**
 * The 30-second rule (Architecture/08 §1): `auto` runs push-based and sub-30-second pipelines
 * in the resident runner, everything slower as a CronJob. An explicit class always wins.
 */
export function executionClass(spec: PipelineSpec): "resident" | "scheduled" {
  if (spec.class === "resident" || spec.class === "scheduled") {
    return spec.class;
  }
  if (spec.schedule) {
    return "scheduled";
  }
  const seconds = periodSeconds(spec.period);
  return seconds === undefined || seconds < 30 ? "resident" : "scheduled";
}

/**
 * Live counters of one resident stream, polled while the pipeline is running. The runner
 * reports totals, so the rate is the difference between two polls; a runner that is down
 * leaves the row without numbers rather than turning it red (the manifest is still fine).
 */
function StreamMetrics({
  project,
  name,
  running,
}: {
  project: string;
  name: string;
  running: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const previous = useRef<{ received: number; at: number } | null>(null);

  const metrics = useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", name), "metrics"],
    enabled: running,
    refetchInterval: 5_000,
    retry: false,
    queryFn: async () => {
      const data = await unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/metrics", {
          params: { path: { project, name } },
        }),
      );
      // Sampled here rather than during render: the fetch is the event that produces a new
      // pair, and a ref written in render would lose the pair on every re-render.
      const at = Date.now();
      const before = previous.current;
      const received = data.received ?? null;
      const rate =
        received !== null && before && at > before.at
          ? ((received - before.received) / (at - before.at)) * 1000
          : null;
      if (received !== null) {
        previous.current = { received, at };
      }
      return { ...data, rate };
    },
  });

  if (!running) {
    return <span className="text-caption text-fg-subtle">{t("pipelines.metrics.paused")}</span>;
  }
  if (metrics.isError || (metrics.isFetched && !metrics.data)) {
    return (
      <span className="text-caption text-fg-subtle">{t("pipelines.metrics.unavailable")}</span>
    );
  }
  if (!metrics.data) {
    return <span className="text-caption text-fg-subtle">{t("app.loading")}</span>;
  }

  const { received, errors, latencyP99Ms, rate } = metrics.data;
  return (
    <dl className="grid grid-cols-[auto_auto] gap-x-2 gap-y-0.5 text-caption">
      {rate !== null ? (
        <>
          <dt className="text-fg-subtle">{t("pipelines.metrics.rate")}</dt>
          <dd className="font-medium tabular-nums text-fg">{rate.toFixed(1)}</dd>
        </>
      ) : null}
      {received != null ? (
        <>
          <dt className="text-fg-subtle">{t("pipelines.metrics.received")}</dt>
          <dd className="tabular-nums text-fg">{received.toLocaleString()}</dd>
        </>
      ) : null}
      {errors != null ? (
        <>
          <dt className="text-fg-subtle">{t("pipelines.metrics.errors")}</dt>
          <dd className={errors > 0 ? "font-medium tabular-nums text-danger" : "tabular-nums text-fg"}>
            {errors.toLocaleString()}
          </dd>
        </>
      ) : null}
      {latencyP99Ms != null ? (
        <>
          <dt className="text-fg-subtle">{t("pipelines.metrics.latency")}</dt>
          <dd className="tabular-nums text-fg">{latencyP99Ms.toFixed(1)}</dd>
        </>
      ) : null}
    </dl>
  );
}

const COLUMNS = 6;

/** Ingestion pipelines of one project: what runs where, how it is doing, and its credentials. */
export function PipelinesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const usage = useProjectUsage(project);
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The assistant may have sent the person here with a form in hand (UI-45), or with a change
  // to one pipeline (`?edit=`, AG-77): taken once, before the first render, so the editor is
  // open from the start and a reload starts clean.
  const [request, setRequest] = useState(() => takeEditRequest());
  // A draft the assistant or another window left (`?draft=`, AG-61): the editor opens on it.
  const [urlDraftName] = useState(() =>
    typeof window === "undefined"
      ? undefined
      : (new URLSearchParams(window.location.search).get("draft") ?? undefined),
  );
  const [editing, setEditing] = useState<Manifest | null>(() => (request?.manifest as Manifest | null) ?? null);
  const [initial] = useState(() =>
    request ? undefined : ((takePrefill(window.location.pathname) as PipelineForm | null) ?? undefined),
  );
  const [dialogOpen, setDialogOpen] = useState(
    initial !== undefined || editing !== null || urlDraftName !== undefined,
  );
  const [formError, setFormError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "pipelines"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "pipelines" } },
        }),
      ),
  });

  // Pausing is a repository change like any other, never a live switch (CC-35).
  const toggle = useMutation({
    mutationFn: async ({ pipeline, run }: { pipeline: Manifest; run: boolean }) => {
      setError(null);
      // `status` is the API's own computation (MF-04); a write never sends it back.
      const body = {
        apiVersion: pipeline.apiVersion,
        kind: pipeline.kind,
        metadata: pipeline.metadata,
        spec: { ...pipeline.spec, enabled: run },
      } as never;
      // The gate wants a green check for exactly this manifest, whatever door it came through
      // (PF-57): a click that only wrote was answered with "The manifest has not been checked"
      // and changed nothing, on every strict installation (T-2264).
      return proposeChecked(project, "pipelines", body, false);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "pipelines") });
    },
    onError: (err) => {
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : t("app.error.generic"),
      );
    },
  });

  /**
   * A create is a POST, an edit a PUT at the manifest's own path; either one lands in the
   * forge and in Approvals like every other write (AP-13). Every signed-in user may propose:
   * the approval, not the button, is the gate.
   */
  const propose = useMutation({
    mutationFn: async ({
      envelope,
      name,
      draft,
    }: {
      envelope: ReturnType<typeof toEnvelope>;
      name: string | null;
      /** The draft the form edited, so the proposal is taken from what was checked (AG-62). */
      draft?: { kind: string; name: string };
    }) => {
      setFormError(null);
      const body = (draft ? { ...envelope, draft } : envelope) as never;
      return unwrap(
        name === null
          ? await api.POST("/api/v1/projects/{project}/{plural}", {
              params: { path: { project, plural: "pipelines" } },
              body,
            })
          : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
              params: { path: { project, plural: "pipelines", name } },
              body,
            }),
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      setEditing(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "pipelines") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : t("app.error.generic"),
      );
    },
  });

  // A link to one pipeline's editor without a change in hand opens the stored manifest once the
  // list has it.
  const requested =
    request && !request.manifest
      ? asManifests(list.data?.items ?? []).find((pipeline) => pipeline.metadata.name === request.name)
      : undefined;
  if (requested) {
    setRequest(null);
    openEditor(requested);
  }

  function openEditor(pipeline: Manifest | null) {
    setEditing(pipeline);
    setFormError(null);
    setDialogOpen(true);
  }

  const head = (
    <TableHead>
      <TableHeaderCell>{t("pipelines.field.name")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("pipelines.field.class")}</TableHeaderCell>
      <TableHeaderCell>{t("pipelines.field.phase")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("pipelines.field.stream")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("pipelines.field.secrets")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
    </TableHead>
  );

  const pipelines = asManifests(list.data?.items ?? []);
  // The reason before the person types: a project at its resident-pipeline quota refuses the next
  // one, and until T-1594 it said so only after the form was filled and submitted. The numbers are
  // the API's own (PF-73, PF-75); the page only reads them. The control stays enabled, because a
  // scheduled pipeline does not count against this dimension and is still allowed.
  const resident = usage.data?.residentPipelines;
  const residentFull =
    resident?.limit !== undefined && (resident.used ?? 0) >= resident.limit;

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("pipelines.title")}
        description={t("pipelines.lead")}
        actions={
          <PermissionGuard project={project} kind="Pipeline" verb="propose">
            <Button
              variant="primary"
              icon={<Icon name="plus" className="size-4" />}
              onClick={() => openEditor(null)}
            >
              {t("pipelines.add")}
            </Button>
          </PermissionGuard>
        }
      />

      {residentFull ? (
        <Alert role="status" tone="warning">
          {t("quota.dimensionExceeded", {
            limit: resident?.limit,
            dimension: t("quota.dimension.residentPipelines"),
          })}
        </Alert>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}

      {/* An empty list is where a person decides what to do next, so it says what the
          two ways in are rather than only that there is nothing here (UI-01). */}
      <ResourceList
        query={list}
        caption={t("pipelines.title")}
        head={head}
        columns={COLUMNS}
        count={pipelines.length}
        empty={
          <EmptyState
            bare
            icon="pipelines"
            title={t("pipelines.empty")}
            description={t("pipelines.emptyHint")}
            action={
              <PermissionGuard project={project} kind="Pipeline" verb="propose">
                <Button variant="primary" onClick={() => openEditor(null)}>
                  {t("pipelines.add")}
                </Button>
              </PermissionGuard>
            }
          />
        }
      >
        {pipelines.map((pipeline) => {
          const spec = pipeline.spec as PipelineSpec;
          const klass = executionClass(spec);
          // Absent means running: a pipeline is only paused by an explicit `false`.
          const running = spec.enabled !== false;
          return (
            <TableRow key={pipeline.metadata.name}>
              <TableCell primary>
                <div>{localized(pipeline.metadata.title, locale, pipeline.metadata.name)}</div>
                {pipeline.metadata.title ? (
                  <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                    {pipeline.metadata.name}
                  </div>
                ) : null}
              </TableCell>
              <TableCell secondary>
                <Badge tone={klass === "resident" ? "info" : "neutral"} title={t(`pipelines.class.${klass}Help`)}>
                  {t(`pipelines.class.${klass}`)}
                </Badge>
              </TableCell>
              <TableCell>
                <LifecycleBadge kind="phase" value={pipeline.status?.phase} />
              </TableCell>
              <TableCell secondary>
                <StreamMetrics
                  project={project}
                  name={pipeline.metadata.name}
                  running={running && klass === "resident"}
                />
              </TableCell>
              <TableCell secondary>
                {/* Names only: a secret's value is resolved by the reconciler and never
                    leaves the cluster, so there is nothing here to mask (PL-17). */}
                <ul className="flex flex-col gap-1">
                  {(spec.secretRefs ?? []).map((ref) => (
                    <li key={`${ref.name}/${ref.key}`}>
                      <Badge mono>
                        {ref.name}/{ref.key}
                        {ref.envVar ? ` → $${ref.envVar}` : ""}
                      </Badge>
                    </li>
                  ))}
                </ul>
              </TableCell>
              <TableCell align="right">
                <div className="flex items-center justify-end gap-1.5">
                  {/* Running or paused is what a person comes to this row for; the rest is in the
                      menu at its end (T-2287). */}
                  <ResourceRowActions
                    project={project}
                    target={{
                      project,
                      kind: "Pipeline",
                      plural: "pipelines",
                      name: pipeline.metadata.name,
                    }}
                    onEdit={() => openEditor(pipeline)}
                    primary={
                      <Button
                        size="sm"
                        disabled={toggle.isPending}
                        onClick={() => toggle.mutate({ pipeline, run: !running })}
                        title={t(running ? "pipelines.pauseHint" : "pipelines.resumeHint")}
                      >
                        {t(running ? "pipelines.pause" : "pipelines.resume")}
                      </Button>
                    }
                  />
                  {pipeline.status?.sourceUrl ? (
                    <SourceLink href={pipeline.status.sourceUrl} label={t("spaces.field.source")} />
                  ) : null}
                </div>
              </TableCell>
            </TableRow>
          );
        })}
      </ResourceList>

      {dialogOpen ? (
        <PipelineEditorDialog
          project={project}
          onOpenChange={(open) => {
            if (!open) {
              setDialogOpen(false);
              setEditing(null);
            }
          }}
          editing={editing}
          initial={initial}
          pending={propose.isPending}
          error={formError}
          draftName={urlDraftName}
          onSubmit={(envelope, draft) =>
            propose.mutate({ envelope, name: editing ? editing.metadata.name : null, draft })
          }
        />
      ) : null}
    </div>
  );
}
