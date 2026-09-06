import { useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ChangeNotice } from "../components/ChangeNotice";

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
    return <span className="text-sm text-surface-fg/60">{t("pipelines.metrics.paused")}</span>;
  }
  if (metrics.isError || (metrics.isFetched && !metrics.data)) {
    return (
      <span className="text-sm text-surface-fg/60">{t("pipelines.metrics.unavailable")}</span>
    );
  }
  if (!metrics.data) {
    return <span className="text-sm text-surface-fg/60">{t("app.loading")}</span>;
  }

  const { received, errors, latencyP99Ms, rate } = metrics.data;
  return (
    <dl className="space-y-0.5 text-sm">
      {rate !== null ? (
        <div className="flex gap-1">
          <dt className="text-surface-fg/60">{t("pipelines.metrics.rate")}</dt>
          <dd className="font-medium tabular-nums">{rate.toFixed(1)}</dd>
        </div>
      ) : null}
      {received != null ? (
        <div className="flex gap-1">
          <dt className="text-surface-fg/60">{t("pipelines.metrics.received")}</dt>
          <dd className="tabular-nums">{received.toLocaleString()}</dd>
        </div>
      ) : null}
      {errors != null ? (
        <div className="flex gap-1">
          <dt className="text-surface-fg/60">{t("pipelines.metrics.errors")}</dt>
          <dd className={errors > 0 ? "font-medium text-danger tabular-nums" : "tabular-nums"}>
            {errors.toLocaleString()}
          </dd>
        </div>
      ) : null}
      {latencyP99Ms != null ? (
        <div className="flex gap-1">
          <dt className="text-surface-fg/60">{t("pipelines.metrics.latency")}</dt>
          <dd className="tabular-nums">{latencyP99Ms.toFixed(1)}</dd>
        </div>
      ) : null}
    </dl>
  );
}

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

  const pipelines = asManifests(list.data.items ?? []);

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-bold">{t("pipelines.title")}</h1>

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {error ? (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : null}

      {pipelines.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("pipelines.empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left text-sm">
            <caption className="sr-only">{t("pipelines.title")}</caption>
            <thead>
              <tr className="border-b border-border bg-surface-subtle">
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("pipelines.field.name")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("pipelines.field.class")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("pipelines.field.phase")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("pipelines.field.stream")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("pipelines.field.secrets")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium text-right">
                  {t("approvals.actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {pipelines.map((pipeline) => {
                const spec = pipeline.spec as PipelineSpec;
                const klass = executionClass(spec);
                // Absent means running: a pipeline is only paused by an explicit `false`.
                const running = spec.enabled !== false;
                return (
                  <tr key={pipeline.metadata.name} className="hover:bg-surface-subtle/50">
                    <td className="px-4 py-3 align-top">
                      <div className="font-medium">
                        {localized(pipeline.metadata.title, locale, pipeline.metadata.name)}
                      </div>
                      {pipeline.metadata.title ? (
                        <div className="font-mono text-xs text-surface-fg/60">
                          {pipeline.metadata.name}
                        </div>
                      ) : null}
                    </td>
                    <td className="px-4 py-3 align-top">
                      <span
                        title={t(`pipelines.class.${klass}Help`)}
                        className="inline-flex items-center rounded border border-border bg-surface-subtle px-2 py-0.5 text-xs font-medium"
                      >
                        {t(`pipelines.class.${klass}`)}
                      </span>
                    </td>
                    <td className="px-4 py-3 align-top">
                      <LifecycleBadge kind="phase" value={pipeline.status?.phase} />
                    </td>
                    <td className="px-4 py-3 align-top">
                      <StreamMetrics
                        project={project}
                        name={pipeline.metadata.name}
                        running={running && klass === "resident"}
                      />
                    </td>
                    <td className="px-4 py-3 align-top">
                      {/* Names only: a secret's value is resolved by the reconciler and never
                          leaves the cluster, so there is nothing here to mask (PL-17). */}
                      <ul className="space-y-1">
                        {(spec.secretRefs ?? []).map((ref) => (
                          <li
                            key={`${ref.name}/${ref.key}`}
                            className="inline-flex items-center rounded border border-border px-2 py-0.5 font-mono text-xs"
                          >
                            {ref.name}/{ref.key}
                            {ref.envVar ? ` → $${ref.envVar}` : ""}
                          </li>
                        ))}
                      </ul>
                    </td>
                    <td className="px-4 py-3 align-top">
                      <div className="flex flex-wrap items-center justify-end gap-2">
                        <button
                          type="button"
                          disabled={toggle.isPending}
                          onClick={() => toggle.mutate({ pipeline, run: !running })}
                          title={t(running ? "pipelines.pauseHint" : "pipelines.resumeHint")}
                          className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50"
                        >
                          {t(running ? "pipelines.pause" : "pipelines.resume")}
                        </button>
                        {pipeline.status?.sourceUrl ? (
                          <a
                            href={pipeline.status.sourceUrl}
                            target="_blank"
                            rel="noreferrer"
                            className="text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
                          >
                            {t("spaces.field.source")}
                          </a>
                        ) : null}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
