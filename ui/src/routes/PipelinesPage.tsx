import { useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ChangeNotice } from "../components/ChangeNotice";
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
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "pipelines"),
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
      return unwrap(
        await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
          params: {
            path: { project, plural: "pipelines", name: pipeline.metadata.name },
          },
          body,
        }),
      );
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

  const head = (
    <TableHead>
      <TableHeaderCell>{t("pipelines.field.name")}</TableHeaderCell>
      <TableHeaderCell>{t("pipelines.field.class")}</TableHeaderCell>
      <TableHeaderCell>{t("pipelines.field.phase")}</TableHeaderCell>
      <TableHeaderCell>{t("pipelines.field.stream")}</TableHeaderCell>
      <TableHeaderCell>{t("pipelines.field.secrets")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
    </TableHead>
  );

  if (list.isPending) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("pipelines.title")} description={t("pipelines.lead")} />
        <Table caption={t("pipelines.title")} status={t("app.loading")}>
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
        <PageHeader title={t("pipelines.title")} description={t("pipelines.lead")} />
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

  const pipelines = asManifests(list.data.items ?? []);

  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("pipelines.title")} description={t("pipelines.lead")} />

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}

      <Table caption={t("pipelines.title")}>
        {head}
        <TableBody>
          {pipelines.length === 0 ? (
            <TableEmpty columns={COLUMNS}>
              <EmptyState bare icon="pipelines" title={t("pipelines.empty")} />
            </TableEmpty>
          ) : (
            pipelines.map((pipeline) => {
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
                  <TableCell>
                    <Badge tone={klass === "resident" ? "info" : "neutral"} title={t(`pipelines.class.${klass}Help`)}>
                      {t(`pipelines.class.${klass}`)}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge kind="phase" value={pipeline.status?.phase} />
                  </TableCell>
                  <TableCell>
                    <StreamMetrics
                      project={project}
                      name={pipeline.metadata.name}
                      running={running && klass === "resident"}
                    />
                  </TableCell>
                  <TableCell>
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
                    <div className="flex flex-wrap items-center justify-end gap-1.5">
                      <Button
                        size="sm"
                        disabled={toggle.isPending}
                        onClick={() => toggle.mutate({ pipeline, run: !running })}
                        title={t(running ? "pipelines.pauseHint" : "pipelines.resumeHint")}
                      >
                        {t(running ? "pipelines.pause" : "pipelines.resume")}
                      </Button>
                      {pipeline.status?.sourceUrl ? (
                        <a
                          href={pipeline.status.sourceUrl}
                          target="_blank"
                          rel="noreferrer"
                          className={buttonClass("ghost", "sm", "text-primary")}
                        >
                          {t("spaces.field.source")}
                          <Icon name="external" className="size-3.5" />
                        </a>
                      ) : null}
                    </div>
                  </TableCell>
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>
    </div>
  );
}
