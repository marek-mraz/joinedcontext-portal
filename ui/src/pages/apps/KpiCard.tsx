import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { readCsrfToken } from "../../api/client";
import { Badge } from "../../components/ui/Badge";
import { Button, buttonClass } from "../../components/ui/Button";

/**
 * An indicator the assistant computed (UI-17, PF-54, PF-55): the value large, its unit, the
 * formula and how many entities it folded, and the one action that matters: writing it into
 * the project's indicator space, through that space's Endpoint, with the person's own session.
 * Nothing is written until the person clicks; the Policy of the indicator space decides.
 * Every string comes from the run's stream and is drawn as text (AG-46).
 */

export interface Kpi {
  name: string;
  title?: string;
  value: number;
  unit?: string;
  formula: string;
  count: number;
  space: string;
  endpointSlug?: string;
  endpointName?: string;
  entity: Record<string, unknown>;
}

/** The indicator of a `compute_kpi` step, or none when the payload is not what the Portal wrote. */
export function kpiOf(output: unknown): Kpi | null {
  if (typeof output !== "object" || output === null) {
    return null;
  }
  const value = output as Record<string, unknown>;
  if (
    typeof value.name !== "string" ||
    typeof value.value !== "number" ||
    typeof value.formula !== "string" ||
    typeof value.space !== "string" ||
    typeof value.entity !== "object" ||
    value.entity === null
  ) {
    return null;
  }
  return {
    name: value.name,
    title: typeof value.title === "string" ? value.title : undefined,
    value: value.value,
    unit: typeof value.unit === "string" && value.unit !== "" ? value.unit : undefined,
    formula: value.formula,
    count: typeof value.count === "number" ? value.count : 0,
    space: value.space,
    endpointSlug: typeof value.endpointSlug === "string" ? value.endpointSlug : undefined,
    endpointName: typeof value.endpointName === "string" ? value.endpointName : undefined,
    entity: value.entity as Record<string, unknown>,
  };
}

/** `18.4`, `1 200`, `0.0031`: enough digits to read, none to mislead. */
export function formatValue(value: number, locale: string): string {
  const digits = Number.isInteger(value) ? 0 : Math.abs(value) >= 100 ? 1 : Math.abs(value) >= 1 ? 2 : 4;
  return new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value);
}

type State = { kind: "idle" } | { kind: "writing" } | { kind: "written" } | { kind: "refused"; detail: string };

export function KpiCard({ project, kpi }: { project: string; kpi: Kpi }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [state, setState] = useState<State>({ kind: "idle" });

  const write = async (): Promise<void> => {
    if (!kpi.endpointSlug) {
      return;
    }
    setState({ kind: "writing" });
    try {
      const response = await fetch(`/api/endpoint/${encodeURIComponent(kpi.endpointSlug)}/ngsi-ld/v1/entities`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/ld+json", "x-csrf-token": readCsrfToken() ?? "" },
        body: JSON.stringify(kpi.entity),
      });
      if (response.ok) {
        setState({ kind: "written" });
        return;
      }
      const problem = (await response.json().catch(() => null)) as { detail?: string; title?: string } | null;
      setState({ kind: "refused", detail: problem?.detail ?? problem?.title ?? t("agentRun.kpi.refused", { status: response.status }) });
    } catch {
      setState({ kind: "refused", detail: t("agentRun.kpi.refused", { status: 0 }) });
    }
  };

  return (
    <section
      aria-label={t("agentRun.kpi.title")}
      className="flex flex-col gap-2 rounded-lg border border-border bg-surface p-3 text-sm"
    >
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="purple">KPI</Badge>
        <span className="min-w-0 break-words font-medium">{kpi.title ?? kpi.name}</span>
      </div>
      <p className="flex flex-wrap items-baseline gap-2">
        <span className="text-3xl font-semibold tabular-nums" data-testid="kpi-value">
          {formatValue(kpi.value, i18n.language)}
        </span>
        {kpi.unit ? <Badge mono>{kpi.unit}</Badge> : null}
      </p>
      <dl className="grid gap-1 text-xs sm:grid-cols-[auto_1fr] sm:gap-x-3">
        <dt className="text-fg-muted">{t("agentRun.kpi.formula")}</dt>
        <dd className="break-words font-mono">{kpi.formula}</dd>
        <dt className="text-fg-muted">{t("agentRun.kpi.count")}</dt>
        <dd>{t("agentRun.kpi.entities", { count: kpi.count })}</dd>
        <dt className="text-fg-muted">{t("agentRun.kpi.id")}</dt>
        <dd className="break-all font-mono">{String(kpi.entity.id ?? kpi.name)}</dd>
      </dl>
      <p className="text-xs text-fg-muted">{t("agentRun.kpi.lead", { space: kpi.space })}</p>
      {state.kind === "refused" ? (
        <p role="alert" className="text-xs text-danger">
          {state.detail}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        {kpi.endpointSlug ? (
          state.kind === "written" ? (
            <>
              <span className="text-xs text-success">{t("agentRun.kpi.written")}</span>
              <Link
                to="/projects/$project/explore"
                params={{ project }}
                search={{ space: kpi.space, endpoint: kpi.endpointName }}
                className={buttonClass("primary", "sm")}
              >
                {t("agentRun.kpi.view")}
              </Link>
            </>
          ) : (
            <Button
              variant="primary"
              size="sm"
              disabled={state.kind === "writing"}
              onClick={() => {
                void write();
              }}
            >
              {state.kind === "writing" ? t("agentRun.kpi.writing") : t("agentRun.kpi.write")}
            </Button>
          )
        ) : (
          <>
            <span className="text-xs text-fg-muted">{t("agentRun.kpi.noEndpoint", { space: kpi.space })}</span>
            <Link
              to="/projects/$project/$plural"
              params={{ project, plural: "endpoints" }}
              className={buttonClass("secondary", "sm")}
            >
              {t("agentRun.kpi.openEndpoints")}
            </Link>
          </>
        )}
      </div>
    </section>
  );
}
