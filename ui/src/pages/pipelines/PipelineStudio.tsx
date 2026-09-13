import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, readCsrfToken, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useBranding } from "../../branding";
import { AccessPanel, deniedAttributes, useAccess } from "../../components/entities/AccessPanel";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { fetchEntities, filterSlotsOf } from "../../components/entities/filters";
import type { Entity, EntityQuery, FilterSlot } from "../../components/entities/filters";
import { Alert, Button, Field, Input, Select } from "../../components/ui";
import { entityTypesOf, pickReadEndpoint, spaceOf } from "../spaces/SpaceInside";
import type { PipelineForm } from "./PipelineEditor";
import { PipelineFlow, setComputeKind } from "./PipelineFlow";
import type { FlowNode } from "./PipelineFlow";
import { PipelineTest } from "./PipelineTest";
import type { Trace } from "./PipelineTest";

/** How many rows one sample shows: enough to tick a handful, small enough to read. */
export const SAMPLE_LIMIT = 20;
/** A page read once an hour when the author has not said otherwise: 3600 s is `scheduled`. */
const DEFAULT_PERIOD = "1h";

export type SourceKind = "none" | "datasource" | "space";
export type Aggregate = "sum" | "average" | "count";
export const AGGREGATES: Aggregate[] = ["sum", "average", "count"];

/** One row of a keyValues sample: the id plus whatever attributes the entity carries. */
export type SampleRow = Entity;

export type StudioPreset = "load" | "kpi";

/** Converts a duration like "15m", "1h", "30s" to an ISO 8601 duration string. */
export function toIsoDuration(period: string): string {
  const trimmed = period.trim();
  if (trimmed.startsWith("P")) {
    return trimmed;
  }
  const match = /^(\d+)(ms|s|m|h|d)$/i.exec(trimmed);
  if (!match) {
    return "PT15M";
  }
  const count = match[1];
  const unit = match[2].toLowerCase();
  switch (unit) {
    case "s":
      return `PT${count}S`;
    case "m":
      return `PT${count}M`;
    case "h":
      return `PT${count}H`;
    case "d":
      return `P${count}D`;
    default:
      return "PT15M";
  }
}

/** Generates standard KPI indicator Bloblang conforming to Architecture/08. */
export function kpiBloblang(params: {
  kpiName: string;
  project: string;
  sourceEndpoint: string;
  sourceSpace: string;
  type: string;
  attribute: string;
  aggregate: "average" | "sum" | "count";
  period: string;
}): string {
  const { kpiName, project, sourceEndpoint, sourceSpace, type, attribute, aggregate, period } =
    params;
  const isoDuration = toIsoDuration(period);
  const formula =
    aggregate === "count"
      ? `count(${type})`
      : `${aggregate === "average" ? "avg" : "sum"}(${attribute}) over ${type}`;

  const valueExpr =
    aggregate === "count"
      ? "this.length()"
      : `if $stations.length() == 0 { 0 } else { $stations.map_each(s -> s.${attribute}.value).sum() / $stations.length() }`;

  const filterLine =
    aggregate === "count"
      ? ""
      : `let stations = this.filter(s -> s.${attribute}.value.type() == "number")\n`;

  return [
    `let domain = env("JC_ORG_DOMAIN")`,
    filterLine.trimEnd(),
    // Whole seconds: the broker refuses an observedAt with a nanosecond fraction, and a 15-minute
    // indicator does not need one (Architecture/08).
    `let now = now().ts_format("2006-01-02T15:04:05Z")`,
    `root.id = "urn:ngsi-ld:KeyPerformanceIndicator:%v:${project}-kpi:${kpiName}".format($domain)`,
    `root.type = "KeyPerformanceIndicator"`,
    `root.name = { "type": "Property", "value": "${kpiName}" }`,
    `root.calculationFormula = { "type": "Property", "value": "${formula}" }`,
    `root.currentValue = {`,
    `  "type": "Property",`,
    `  "value": ${valueExpr},`,
    `  "unitCode": "C62",`,
    `  "observedAt": $now`,
    `}`,
    `root.calculationPeriod = { "type": "Property", "value": { "start": $now.ts_sub_iso8601("${isoDuration}"), "end": $now } }`,
    `root.updatedAt = { "type": "Property", "value": { "@type": "DateTime", "@value": $now } }`,
    `root.derivedFrom = { "type": "Relationship", "object": "urn:ngsi-ld:Endpoint:%v:${sourceSpace}:${sourceEndpoint}".format($domain) }`,
    `root.computedBy = { "type": "Relationship", "object": "urn:ngsi-ld:Pipeline:%v:${project}:${kpiName}".format($domain) }`,
    "",
  ]
    .filter((line) => line !== "")
    .join("\n");
}

/** Finds the first Endpoint targeting a KPI space (`{project}-kpi`) to write indicators to. */
export function findKpiTargetEndpoint(
  project: string,
  endpoints: Manifest[],
  orgDomain?: string,
): string | undefined {
  const kpiSpace = `${project}-kpi`;
  const ep = endpoints.find((e) => spaceOf(e) === kpiSpace || spaceOf(e)?.endsWith("-kpi"));
  if (!ep) {
    return undefined;
  }
  const space = spaceOf(ep) ?? kpiSpace;
  const domain = orgDomain || "local";
  return `urn:ngsi-ld:Endpoint:${domain}:${space}:${ep.metadata.name}`;
}

/** Where the form's source points: a feed, a space, or nowhere yet. */
export function sourceKindOf(form: PipelineForm | undefined): SourceKind {
  if (form?.source?.dataSourceRef) {
    return "datasource";
  }
  if (form?.source?.endpointRef) {
    return "space";
  }
  return "none";
}

/** The attributes of one class of an inline LinkML model, `[]` when the model is not inline. */
export function attributesOf(model: Manifest | undefined, type: string | undefined): string[] {
  return filterSlotsOf(model, type).map((slot) => slot.name);
}

/**
 * The Bloblang of one aggregate over the page the fetch returns (PL-41): the page is one
 * message holding the entities, so the mapping folds it into one derived entity and hands the
 * output writer an array, the shape `entityOperations/upsert` takes.
 */
export function aggregateBloblang(
  aggregate: Aggregate,
  input: { type: string; attribute: string; space: string; outputType: string },
): string {
  const { type, attribute, space, outputType } = input;
  const values = `this.map_each(e -> e.${attribute}.value.number().catch(0))`;
  const value = {
    sum: `${values}.sum()`,
    average: `if this.length() == 0 { 0 } else { ${values}.sum() / this.length() }`,
    count: "this.length()",
  }[aggregate];
  const property = `${attribute}${aggregate[0].toUpperCase()}${aggregate.slice(1)}`;
  return [
    `# ${aggregate} of ${attribute} over the ${type} entities the fetch returned (PL-41, PL-36)`,
    `let domain = env("JC_ORG_DOMAIN")`,
    `let seen = now().ts_format("2006-01-02T15:04:05Z")`,
    `root = [{`,
    `  "id": "urn:ngsi-ld:%v:%v:%v:%v".format("${outputType}", $domain, "${space}", "${attribute}-${aggregate}"),`,
    `  "type": "${outputType}",`,
    `  "${property}": { "type": "Property", "value": ${value}, "observedAt": $seen },`,
    `  "derivedFrom": { "type": "Relationship", "object": this.map_each(e -> e.id) }`,
    `}]`,
    "",
  ].join("\n");
}

function cell(value: unknown): string {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > 32 ? `${text.slice(0, 31)}…` : text;
}

export interface PipelineStudioProps {
  project: string;
  draft: PipelineForm | undefined;
  onChange: (form: PipelineForm) => void;
  dataSources: Manifest[];
  endpoints: Manifest[];
  /** The manifest the form is right now, for the sample test (PL-43); none hides the test. */
  toManifest?: (form: PipelineForm) => unknown;
  /** The test's answer for the mapping it ran (PL-49). */
  onVerdict?: (ok: boolean, bloblang: string) => void;
}

/** The URL of the `http` DataSource the draft reads, if it reads one (PL-48). */
export function sampleUrlOf(
  draft: PipelineForm | undefined,
  dataSources: Manifest[],
  endpoints: Manifest[] = [],
): string | undefined {
  const name = draft?.source?.dataSourceRef;
  const source = name
    ? dataSources.find((candidate) => candidate.metadata.name === name)
    : undefined;
  if (source) {
    if (source.spec.type !== "http") {
      return undefined;
    }
    const url = (source.spec.http as { url?: unknown } | undefined)?.url;
    return typeof url === "string" && /^https?:\/\//.test(url) ? url : undefined;
  }
  // An endpoint-sourced pipeline tests on a page of that endpoint, read the way the
  // reconciler will read it (PL-45): the runner fetches the URL under its own policy.
  const endpointName = draft?.source?.endpointRef;
  const endpoint = endpointName
    ? endpoints.find((candidate) => candidate.metadata.name === endpointName)
    : undefined;
  const slug = (endpoint?.spec as { slug?: unknown } | undefined)?.slug;
  const type = draft?.source?.query?.type;
  if (typeof slug !== "string" || !slug || typeof type !== "string" || !type) {
    return undefined;
  }
  const params = new URLSearchParams({ type, limit: "1000" });
  const attrs = draft?.source?.query?.attrs;
  if (Array.isArray(attrs) && attrs.length > 0) {
    params.set("attrs", attrs.join(","));
  }
  return `${window.location.origin}/api/endpoint/${slug}/ngsi-ld/v1/entities?${params.toString()}`;
}

/**
 * The studio above the pipeline form (UI-32): source, entities with a live sample to tick,
 * and an aggregate snippet. Every choice is written into the same form the fields below
 * and the YAML view edit, so nothing here is state of its own except the sample.
 */
export function PipelineStudio({
  project,
  draft,
  onChange,
  dataSources,
  endpoints,
  toManifest,
  onVerdict,
}: PipelineStudioProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const { orgDomain } = useBranding();
  const [sample, setSample] = useState<SampleRow[] | null>(null);
  const [sampleError, setSampleError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [aggregateAttribute, setAggregateAttribute] = useState("");
  const [studioView, setStudioView] = useState<"flow" | "form">("flow");
  const [selectedNode, setSelectedNode] = useState<FlowNode["id"] | null>(() =>
    draft?.compute?.kind ? "compute" : "source",
  );
  const [flowTrace, setFlowTrace] = useState<Trace | null>(null);
  // The kind and the space are the author's choice until the form carries them: a chosen kind
  // with nothing picked yet, or a space with no endpoint, is not in the manifest at all.
  const [kindChoice, setKindChoice] = useState<SourceKind>(() => sourceKindOf(draft));
  const [spaceChoice, setSpaceChoice] = useState<string | undefined>(undefined);

  const [preset, setPreset] = useState<StudioPreset>(() =>
    draft?.output?.type === "KeyPerformanceIndicator" ? "kpi" : "load",
  );

  const draftEndpointName =
    typeof draft?.source?.endpointRef === "object"
      ? ((draft.source.endpointRef as { name?: string })?.name ?? "")
      : (draft?.source?.endpointRef ?? "");

  const [kpiEndpoint, setKpiEndpoint] = useState<string>(draftEndpointName);
  const [kpiType, setKpiType] = useState<string>(
    (draft?.source?.query?.type as string) || "BikeHireDockingStation",
  );
  const [kpiAttribute, setKpiAttribute] = useState<string>(
    (draft?.source?.query?.attrs as string[])?.[0] || "availableBikeNumber",
  );
  const [kpiAggregate, setKpiAggregate] = useState<"average" | "sum" | "count">("average");
  const [kpiPeriod, setKpiPeriod] = useState<string>(draft?.period || "15m");
  const [kpiName, setKpiName] = useState<string>(draft?.name || "bikes-available-avg");
  const [kpiTesting, setKpiTesting] = useState(false);
  const [kpiTestError, setKpiTestError] = useState<string | null>(null);
  const [kpiValue, setKpiValue] = useState<unknown | null>(null);

  function emitKpi(params: {
    endpointName: string;
    type: string;
    attribute: string;
    aggregate: "average" | "sum" | "count";
    period: string;
    name: string;
  }) {
    const ep = endpoints.find((e) => e.metadata.name === params.endpointName);
    const sourceSpace = (ep ? spaceOf(ep) : undefined) ?? project;
    const bloblang = kpiBloblang({
      kpiName: params.name,
      project,
      sourceEndpoint: params.endpointName,
      sourceSpace,
      type: params.type,
      attribute: params.attribute,
      aggregate: params.aggregate,
      period: params.period,
    });
    const targetEndpoint =
      findKpiTargetEndpoint(project, endpoints, orgDomain) ?? draft?.targetEndpoint;
    onChange({
      ...draft,
      name: params.name,
      // The reconciler picks the class from the cadence (PL-26): a period alone is a resident
      // stream that polls, and `scheduled` would demand a cron schedule the preset has not got.
      class: "auto",
      period: params.period,
      source: {
        ...draft?.source,
        endpointRef: params.endpointName,
        query: { type: params.type, attrs: [params.attribute] },
      },
      compute: {
        kind: "bloblang",
        bloblang,
      },
      output: {
        type: "KeyPerformanceIndicator",
        mode: "upsert",
      },
      targetEndpoint,
    });
  }

  async function runKpiTest() {
    if (!kpiEndpoint) {
      return;
    }
    const chosenEndpoint = endpoints.find((e) => e.metadata.name === kpiEndpoint);
    const slug =
      typeof chosenEndpoint?.spec.slug === "string" ? (chosenEndpoint.spec.slug as string) : "";
    if (!slug) {
      setKpiTestError(t("pipelines.test.failed", { status: 0 }));
      return;
    }

    const sourceSpace = (chosenEndpoint ? spaceOf(chosenEndpoint) : undefined) ?? project;
    const bloblang = kpiBloblang({
      kpiName,
      project,
      sourceEndpoint: kpiEndpoint,
      sourceSpace,
      type: kpiType,
      attribute: kpiAttribute,
      aggregate: kpiAggregate,
      period: kpiPeriod,
    });
    const targetEndpoint =
      findKpiTargetEndpoint(project, endpoints, orgDomain) ?? draft?.targetEndpoint;

    const kpiForm: PipelineForm = {
      ...draft,
      name: kpiName,
      class: "auto",
      period: kpiPeriod,
      source: {
        ...draft?.source,
        endpointRef: kpiEndpoint,
        query: { type: kpiType, attrs: [kpiAttribute] },
      },
      compute: {
        kind: "bloblang",
        bloblang,
      },
      output: {
        type: "KeyPerformanceIndicator",
        mode: "upsert",
      },
      targetEndpoint,
    };

    setKpiTesting(true);
    setKpiTestError(null);
    try {
      const origin = typeof window !== "undefined" ? window.location.origin : "";
      const url = `${origin}/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${encodeURIComponent(kpiType)}&attrs=${encodeURIComponent(kpiAttribute)}&limit=1000`;
      const response = await fetch(
        `/api/v1/projects/${encodeURIComponent(project)}/pipelines/test`,
        {
          method: "POST",
          credentials: "same-origin",
          headers: {
            "content-type": "application/json",
            "x-csrf-token": readCsrfToken() ?? "",
          },
          body: JSON.stringify({
            pipeline: toManifest ? toManifest(kpiForm) : kpiForm,
            sample: { url, format: "json" },
          }),
        },
      );
      if (!response.ok) {
        const problem = (await response.json().catch(() => null)) as {
          detail?: string;
        } | null;
        setKpiTestError(problem?.detail ?? t("pipelines.test.failed", { status: response.status }));
        return;
      }
      const answer = (await response.json()) as Trace;
      const firstMapping = answer.mapping?.[0] as Record<string, unknown> | undefined;
      const currentValue = firstMapping?.currentValue as Record<string, unknown> | undefined;
      const val = currentValue?.value ?? null;
      setKpiValue(val);
      const isOk =
        answer.errors.length === 0 &&
        answer.validation.length > 0 &&
        answer.validation.every((v) => v.ok);
      onVerdict?.(isOk, bloblang);
    } catch {
      setKpiTestError(t("pipelines.test.failed", { status: 0 }));
    } finally {
      setKpiTesting(false);
    }
  }

  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  const spaceList = useMemo(() => asManifests(spaces.data?.items ?? []), [spaces.data]);
  const modelList = useMemo(() => asManifests(models.data?.items ?? []), [models.data]);

  const formKind = sourceKindOf(draft);
  const kind = formKind === "none" ? kindChoice : formKind;
  const endpointName = draft?.source?.endpointRef;
  const endpoint = endpoints.find((e) => e.metadata.name === endpointName);
  const space = (endpoint ? spaceOf(endpoint) : undefined) ?? spaceChoice;
  const spaceEndpoints = endpoints.filter((e) => spaceOf(e) === space);
  const spaceManifest = spaceList.find((s) => s.metadata.name === space);
  const model = modelList.find((m) => m.metadata.name === spaceManifest?.spec.dataModelRef);
  const types = model ? entityTypesOf(model) : [];
  const type = draft?.source?.query?.type as string | undefined;
  const slots: FilterSlot[] = filterSlotsOf(model, type);
  if (slots.length === 0) {
    // No inline model: the sample's own keys are the attributes there are.
    for (const row of sample ?? []) {
      for (const key of Object.keys(row)) {
        if (!["id", "type", "@context"].includes(key) && !slots.some((slot) => slot.name === key)) {
          slots.push({ name: key, kind: "Property" });
        }
      }
    }
  }
  const attributes = slots.map((slot) => slot.name);
  const query: EntityQuery = {
    type,
    attrs: draft?.source?.query?.attrs as string[] | undefined,
    q: draft?.source?.query?.q as string | undefined,
    scopeQ: draft?.source?.query?.scopeQ as string | undefined,
  };
  const slug = typeof endpoint?.spec.slug === "string" ? (endpoint.spec.slug as string) : undefined;
  const access = useAccess(slug);
  const denied = deniedAttributes(access.data, type, slots, t);
  const ids = (draft?.source?.query?.ids as string[] | undefined) ?? [];

  function update(patch: (form: PipelineForm) => PipelineForm) {
    onChange(patch(draft ?? { class: "auto" }));
  }

  function setQuery(fields: Record<string, unknown>) {
    update((form) => ({
      ...form,
      source: { ...form.source, query: { ...form.source?.query, ...fields } },
    }));
  }

  function chooseKind(next: SourceKind) {
    setSample(null);
    setKindChoice(next);
    setSpaceChoice(undefined);
    update((form) => ({
      ...form,
      source: next === "none" ? undefined : {},
    }));
  }

  function chooseDataSource(name: string) {
    update((form) => ({
      ...form,
      source: name ? { dataSourceRef: name } : {},
    }));
  }

  function chooseSpace(name: string) {
    setSample(null);
    setSpaceChoice(name || undefined);
    const read = pickReadEndpoint(endpoints.filter((e) => spaceOf(e) === name));
    update((form) => ({
      ...form,
      // A page read once an hour unless the author already chose a cadence (PL-26).
      period: form.period ?? (form.schedule ? undefined : DEFAULT_PERIOD),
      source: read ? { endpointRef: read.metadata.name } : {},
    }));
  }

  function chooseEndpoint(name: string) {
    setSample(null);
    update((form) => ({
      ...form,
      source: { ...form.source, endpointRef: name, query: undefined },
    }));
  }

  function changeQuery(next: EntityQuery) {
    if (next.type !== type) {
      // Another type: the sample and the ticked ids belonged to the old one.
      setSample(null);
      setQuery({ ...next, ids: undefined });
      return;
    }
    setQuery({ ...next });
  }

  function toggleId(id: string, on: boolean) {
    const next = on ? [...ids, id] : ids.filter((i) => i !== id);
    setQuery({ ids: next.length > 0 ? next : undefined });
  }

  async function loadSample() {
    if (!slug || !type) {
      return;
    }
    setLoading(true);
    setSampleError(null);
    try {
      setSample((await fetchEntities(slug, query, { limit: SAMPLE_LIMIT })).rows);
    } catch (err) {
      setSample(null);
      setSampleError(err instanceof ApiError ? err.message : t("app.error.generic"));
    } finally {
      setLoading(false);
    }
  }

  function applyAggregate(aggregate: Aggregate) {
    if (!type || !space) {
      return;
    }
    const attribute = aggregateAttribute || attributes[0] || "value";
    const outputType = (draft?.output?.type as string | undefined) || `${type}Aggregate`;
    update((form) => ({
      ...form,
      compute: {
        kind: "bloblang",
        bloblang: aggregateBloblang(aggregate, {
          type,
          attribute,
          space,
          outputType,
        }),
      },
      output: { type: outputType, mode: form.output?.mode ?? "upsert" },
    }));
  }

  const sectionClass = "flex flex-col gap-3 rounded-md border border-border bg-surface-subtle p-3";

  return (
    <div className="flex flex-col gap-3" data-testid="pipeline-studio">
      <Field id="studio-preset" label={t("pipelines.studio.preset.title")}>
        <Select
          id="studio-preset"
          value={preset}
          onChange={(event) => {
            const next = event.target.value as StudioPreset;
            setPreset(next);
            if (next === "kpi" && kpiEndpoint) {
              emitKpi({
                endpointName: kpiEndpoint,
                type: kpiType,
                attribute: kpiAttribute,
                aggregate: kpiAggregate,
                period: kpiPeriod,
                name: kpiName,
              });
            }
          }}
        >
          <option value="load">{t("pipelines.studio.preset.load")}</option>
          <option value="kpi">{t("pipelines.studio.preset.kpi")}</option>
        </Select>
      </Field>

      {preset === "kpi" ? (
        <section
          className={sectionClass}
          aria-labelledby="studio-kpi-title"
          data-testid="studio-kpi-panel"
        >
          <h3 id="studio-kpi-title" className="text-body font-semibold text-fg">
            {t("pipelines.studio.kpi.title")}
          </h3>
          <p className="text-caption text-fg-muted">{t("pipelines.studio.kpi.lead")}</p>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field id="studio-kpi-endpoint" label={t("pipelines.studio.kpi.endpoint")}>
              <Select
                id="studio-kpi-endpoint"
                value={kpiEndpoint}
                onChange={(event) => {
                  const ep = event.target.value;
                  setKpiEndpoint(ep);
                  emitKpi({
                    endpointName: ep,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              >
                <option value="">—</option>
                {endpoints.map((e) => (
                  <option key={e.metadata.name} value={e.metadata.name}>
                    {localized(e.metadata.title, locale, e.metadata.name)}
                  </option>
                ))}
              </Select>
            </Field>
            <Field id="studio-kpi-name" label={t("pipelines.studio.kpi.name")}>
              <Input
                id="studio-kpi-name"
                value={kpiName}
                onChange={(event) => {
                  const name = event.target.value;
                  setKpiName(name);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name,
                  });
                }}
              />
            </Field>
            <Field id="studio-kpi-type" label={t("pipelines.studio.kpi.type")}>
              <Input
                id="studio-kpi-type"
                value={kpiType}
                onChange={(event) => {
                  const nextType = event.target.value;
                  setKpiType(nextType);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: nextType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              />
            </Field>
            <Field id="studio-kpi-attribute" label={t("pipelines.studio.kpi.attribute")}>
              <Input
                id="studio-kpi-attribute"
                value={kpiAttribute}
                onChange={(event) => {
                  const nextAttr = event.target.value;
                  setKpiAttribute(nextAttr);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: nextAttr,
                    aggregate: kpiAggregate,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              />
            </Field>
            <Field id="studio-kpi-agg" label={t("pipelines.studio.kpi.aggregate")}>
              <Select
                id="studio-kpi-agg"
                value={kpiAggregate}
                onChange={(event) => {
                  const nextAgg = event.target.value as "average" | "sum" | "count";
                  setKpiAggregate(nextAgg);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: nextAgg,
                    period: kpiPeriod,
                    name: kpiName,
                  });
                }}
              >
                <option value="average">{t("pipelines.studio.aggregate.average")}</option>
                <option value="sum">{t("pipelines.studio.aggregate.sum")}</option>
                <option value="count">{t("pipelines.studio.aggregate.count")}</option>
              </Select>
            </Field>
            <Field id="studio-kpi-period" label={t("pipelines.studio.kpi.period")}>
              <Input
                id="studio-kpi-period"
                value={kpiPeriod}
                onChange={(event) => {
                  const nextPeriod = event.target.value;
                  setKpiPeriod(nextPeriod);
                  emitKpi({
                    endpointName: kpiEndpoint,
                    type: kpiType,
                    attribute: kpiAttribute,
                    aggregate: kpiAggregate,
                    period: nextPeriod,
                    name: kpiName,
                  });
                }}
              />
            </Field>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <Button
              size="sm"
              variant="primary"
              data-testid="studio-kpi-test"
              disabled={!kpiEndpoint || kpiTesting}
              onClick={() => {
                void runKpiTest();
              }}
            >
              {kpiTesting ? t("pipelines.studio.kpi.testing") : t("pipelines.studio.kpi.test")}
            </Button>
            {kpiValue !== null && kpiValue !== undefined ? (
              <span className="text-caption text-fg">
                {t("pipelines.studio.kpi.computedValue")}:{" "}
                <strong data-testid="studio-kpi-value">{String(kpiValue)}</strong>
              </span>
            ) : null}
          </div>

          {kpiTestError ? (
            <Alert role="alert" tone="danger">
              {kpiTestError}
            </Alert>
          ) : null}
        </section>
      ) : (
        <>
          <section className={sectionClass} aria-labelledby="studio-flow">
            <div className="flex items-center justify-between">
              <h3 id="studio-flow" className="text-body font-semibold text-fg">
                {t("pipelines.flow.canvas", { defaultValue: "Pipeline canvas" })}
              </h3>
              <div
                role="tablist"
                aria-label={t("pipelines.flow.canvas", { defaultValue: "Pipeline canvas" })}
                className="flex items-center gap-1"
              >
                <Button
                  role="tab"
                  size="sm"
                  variant={studioView === "flow" ? "secondary" : "ghost"}
                  aria-selected={studioView === "flow"}
                  data-testid="studio-view-flow"
                  onClick={() => setStudioView("flow")}
                >
                  {t("pipelines.flow.view.flow", { defaultValue: "Flow" })}
                </Button>
                <Button
                  role="tab"
                  size="sm"
                  variant={studioView === "form" ? "secondary" : "ghost"}
                  aria-selected={studioView === "form"}
                  data-testid="studio-view-form"
                  onClick={() => setStudioView("form")}
                >
                  {t("pipelines.flow.view.form", { defaultValue: "Form" })}
                </Button>
              </div>
            </div>

            {studioView === "flow" ? (
              <div className="flex flex-col gap-3">
                <PipelineFlow
                  form={draft}
                  onChange={onChange}
                  trace={flowTrace}
                  selected={selectedNode}
                  onSelect={setSelectedNode}
                  dataSources={dataSources}
                  endpoints={endpoints}
                />

                {selectedNode === "compute" ? (
                  <div
                    className="flex flex-col gap-2 rounded-md border border-border bg-surface p-3"
                    data-testid="flow-node-editor-compute"
                  >
                    <div className="flex items-center justify-between">
                      <span className="text-caption font-semibold text-fg">
                        {t("pipelines.flow.node.compute", { defaultValue: "Compute" })} (
                        {draft?.compute?.kind ??
                          t("pipelines.flow.passThrough", { defaultValue: "pass-through" })}
                        )
                      </span>
                      {draft?.compute?.kind ? (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            onChange(setComputeKind(draft, null));
                          }}
                        >
                          {t("pipelines.flow.remove", { defaultValue: "Remove" })}
                        </Button>
                      ) : null}
                    </div>
                    {draft?.compute?.kind === "bloblang" ? (
                      <Field id="flow-bloblang-field" label={t("pipelines.flow.nodeBloblang")}>
                        <textarea
                          id="flow-bloblang-field"
                          data-testid="flow-bloblang"
                          aria-label={t("pipelines.flow.nodeBloblang")}
                          rows={6}
                          className="focus-ring w-full rounded-md border border-border bg-surface p-2 font-mono text-caption text-fg"
                          value={draft?.compute?.bloblang ?? ""}
                          onChange={(e) => {
                            onChange({
                              ...draft,
                              compute: {
                                ...draft?.compute,
                                kind: "bloblang",
                                bloblang: e.target.value,
                              },
                            });
                          }}
                        />
                      </Field>
                    ) : draft?.compute?.kind === "mapping" ? (
                      <Field id="flow-mapping-field" label={t("pipelines.field.mappingRef")}>
                        <Input
                          id="flow-mapping-field"
                          data-testid="flow-mapping-ref"
                          value={draft?.compute?.mappingRef ?? ""}
                          onChange={(e) => {
                            onChange({
                              ...draft,
                              compute: {
                                ...draft?.compute,
                                kind: "mapping",
                                mappingRef: e.target.value,
                              },
                            });
                          }}
                        />
                      </Field>
                    ) : draft?.compute?.kind === "wasm" || draft?.compute?.kind === "container" ? (
                      <div className="grid gap-2 sm:grid-cols-2">
                        <Field id="flow-module-field" label={t("pipelines.field.module")}>
                          <Input
                            id="flow-module-field"
                            data-testid="flow-module"
                            value={draft?.compute?.module ?? ""}
                            onChange={(e) => {
                              onChange({
                                ...draft,
                                compute: {
                                  ...draft?.compute,
                                  module: e.target.value,
                                },
                              });
                            }}
                          />
                        </Field>
                        <Field id="flow-function-field" label={t("pipelines.field.function")}>
                          <Input
                            id="flow-function-field"
                            data-testid="flow-function"
                            value={draft?.compute?.function ?? ""}
                            onChange={(e) => {
                              onChange({
                                ...draft,
                                compute: {
                                  ...draft?.compute,
                                  function: e.target.value,
                                },
                              });
                            }}
                          />
                        </Field>
                      </div>
                    ) : (
                      <p className="text-caption text-fg-muted">
                        {t("pipelines.flow.passThrough", {
                          defaultValue: "Pass-through (no compute step)",
                        })}
                      </p>
                    )}
                  </div>
                ) : selectedNode === "source" || selectedNode === "output" ? (
                  <p className="text-caption text-fg-muted">
                    {t("pipelines.flow.selectedHint", {
                      defaultValue: "Edit source and output in the sections below.",
                    })}
                  </p>
                ) : null}

                {flowTrace && selectedNode ? (
                  <div className="grid gap-2 sm:grid-cols-2" data-testid="flow-sample-drawer">
                    <div className="flex flex-col gap-1 rounded-md border border-border bg-surface p-2">
                      <span className="text-caption font-semibold text-fg">
                        {t("pipelines.flow.samplesIn", { defaultValue: "Input sample" })}
                      </span>
                      <pre
                        data-testid="flow-sample-in"
                        className="max-h-36 overflow-auto font-mono text-caption whitespace-pre-wrap break-words"
                      >
                        {(() => {
                          const sampleIn =
                            selectedNode === "source" || selectedNode === "compute"
                              ? flowTrace.input?.sample
                              : flowTrace.mapping?.[0];
                          return sampleIn !== undefined
                            ? JSON.stringify(sampleIn, null, 2)
                            : t("pipelines.flow.noSample", { defaultValue: "No sample" });
                        })()}
                      </pre>
                    </div>
                    <div className="flex flex-col gap-1 rounded-md border border-border bg-surface p-2">
                      <span className="text-caption font-semibold text-fg">
                        {t("pipelines.flow.samplesOut", { defaultValue: "Output sample" })}
                      </span>
                      <pre
                        data-testid="flow-sample-out"
                        className="max-h-36 overflow-auto font-mono text-caption whitespace-pre-wrap break-words"
                      >
                        {(() => {
                          const okValidation = flowTrace.validation?.find((v) => v.ok);
                          const sampleOut =
                            selectedNode === "source"
                              ? flowTrace.input?.sample
                              : selectedNode === "compute"
                                ? flowTrace.mapping?.[0]
                                : okValidation !== undefined
                                  ? flowTrace.mapping?.[okValidation.index]
                                  : undefined;
                          return sampleOut !== undefined
                            ? JSON.stringify(sampleOut, null, 2)
                            : t("pipelines.flow.noSample", { defaultValue: "No sample" });
                        })()}
                      </pre>
                    </div>
                  </div>
                ) : null}
              </div>
            ) : null}
          </section>

          {toManifest ? (
            <PipelineTest
              project={project}
              draft={draft}
              onChange={onChange}
              toManifest={toManifest}
              sampleUrl={sampleUrlOf(draft, dataSources, endpoints)}
              onVerdict={onVerdict}
              onTrace={setFlowTrace}
            />
          ) : null}
          <section className={sectionClass} aria-labelledby="studio-source">
            <h3 id="studio-source" className="text-body font-semibold text-fg">
              {t("pipelines.studio.source")}
            </h3>
            <Field id="studio-source-kind" label={t("pipelines.studio.sourceKind")}>
              <Select
                id="studio-source-kind"
                value={kind}
                onChange={(event) => chooseKind(event.target.value as SourceKind)}
              >
                <option value="none">{t("pipelines.studio.kind.none")}</option>
                <option value="datasource">{t("pipelines.studio.kind.datasource")}</option>
                <option value="space">{t("pipelines.studio.kind.space")}</option>
              </Select>
            </Field>
            {kind === "datasource" ? (
              <Field id="studio-datasource" label={t("pipelines.field.dataSource")}>
                <Select
                  id="studio-datasource"
                  value={draft?.source?.dataSourceRef ?? ""}
                  onChange={(event) => chooseDataSource(event.target.value)}
                >
                  <option value="">—</option>
                  {dataSources.map((source) => (
                    <option key={source.metadata.name} value={source.metadata.name}>
                      {localized(source.metadata.title, locale, source.metadata.name)}
                    </option>
                  ))}
                </Select>
              </Field>
            ) : null}
            {kind === "space" ? (
              <div className="grid gap-3 sm:grid-cols-2">
                <Field id="studio-space" label={t("pipelines.studio.space")}>
                  <Select
                    id="studio-space"
                    value={space ?? ""}
                    onChange={(event) => chooseSpace(event.target.value)}
                  >
                    <option value="">—</option>
                    {spaceList.map((s) => (
                      <option key={s.metadata.name} value={s.metadata.name}>
                        {localized(s.metadata.title, locale, s.metadata.name)}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field
                  id="studio-endpoint"
                  label={t("pipelines.studio.readThrough")}
                  description={t("pipelines.studio.readThroughHint")}
                >
                  <Select
                    id="studio-endpoint"
                    value={endpointName ?? ""}
                    disabled={!space}
                    onChange={(event) => chooseEndpoint(event.target.value)}
                  >
                    <option value="">—</option>
                    {spaceEndpoints.map((e) => (
                      <option key={e.metadata.name} value={e.metadata.name}>
                        {localized(e.metadata.title, locale, e.metadata.name)}
                      </option>
                    ))}
                  </Select>
                </Field>
              </div>
            ) : null}
          </section>

          {kind === "space" && space ? (
            <section className={sectionClass} aria-labelledby="studio-entities">
              <h3 id="studio-entities" className="text-body font-semibold text-fg">
                {t("pipelines.studio.entities")}
              </h3>
              <EntityFilters
                id="studio"
                types={types}
                slots={slots}
                value={query}
                onChange={changeQuery}
                denied={denied}
              />
              <AccessPanel slug={slug} type={type} access={access} />
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  size="sm"
                  disabled={!slug || !type || loading}
                  onClick={() => {
                    void loadSample();
                  }}
                >
                  {loading ? t("app.loading") : t("pipelines.studio.loadSample")}
                </Button>
                <span className="text-caption text-fg-muted">
                  {slug
                    ? t("pipelines.studio.sampleHint", { count: SAMPLE_LIMIT })
                    : t("pipelines.studio.noEndpoint")}
                </span>
              </div>
              {sampleError ? (
                <Alert role="alert" tone="danger">
                  {sampleError}
                </Alert>
              ) : null}
              {sample ? (
                sample.length === 0 ? (
                  <p className="text-caption text-fg-muted">{t("pipelines.studio.noEntities")}</p>
                ) : (
                  <div className="max-h-64 overflow-auto rounded-md border border-border bg-surface">
                    <table className="w-full text-left text-caption">
                      <caption className="sr-only">{t("pipelines.studio.sample")}</caption>
                      <thead>
                        <tr className="border-b border-border">
                          <th scope="col" className="px-2 py-1">
                            <span className="sr-only">{t("pipelines.studio.tick")}</span>
                          </th>
                          <th scope="col" className="px-2 py-1 font-medium">
                            id
                          </th>
                          {attributes.slice(0, 4).map((attribute) => (
                            <th
                              key={attribute}
                              scope="col"
                              className="px-2 py-1 font-mono font-medium"
                            >
                              {attribute}
                            </th>
                          ))}
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-border">
                        {sample.map((row) => (
                          <tr key={row.id}>
                            <td className="px-2 py-1">
                              <input
                                type="checkbox"
                                aria-label={row.id}
                                checked={ids.includes(row.id)}
                                onChange={(event) => toggleId(row.id, event.target.checked)}
                              />
                            </td>
                            <td className="px-2 py-1 font-mono">{row.id}</td>
                            {attributes.slice(0, 4).map((attribute) => (
                              <td key={attribute} className="px-2 py-1 font-mono">
                                {attribute in row ? cell(row[attribute]) : ""}
                              </td>
                            ))}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )
              ) : null}
              {ids.length > 0 ? (
                <p className="text-caption text-fg-muted">
                  {t("pipelines.studio.ticked", { count: ids.length })}
                </p>
              ) : null}
            </section>
          ) : null}

          {kind === "space" && type ? (
            <section className={sectionClass} aria-labelledby="studio-process">
              <h3 id="studio-process" className="text-body font-semibold text-fg">
                {t("pipelines.studio.process")}
              </h3>
              <p className="text-caption text-fg-muted">{t("pipelines.studio.processHint")}</p>
              <div className="flex flex-wrap items-end gap-2">
                <Field id="studio-aggregate-attribute" label={t("pipelines.studio.attribute")}>
                  {attributes.length > 0 ? (
                    <Select
                      id="studio-aggregate-attribute"
                      value={aggregateAttribute || attributes[0]}
                      onChange={(event) => setAggregateAttribute(event.target.value)}
                    >
                      {attributes.map((attribute) => (
                        <option key={attribute} value={attribute}>
                          {attribute}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      id="studio-aggregate-attribute"
                      value={aggregateAttribute}
                      onChange={(event) => setAggregateAttribute(event.target.value)}
                    />
                  )}
                </Field>
                {AGGREGATES.map((aggregate) => (
                  <Button key={aggregate} size="sm" onClick={() => applyAggregate(aggregate)}>
                    {t(`pipelines.studio.aggregate.${aggregate}`)}
                  </Button>
                ))}
              </div>
            </section>
          ) : null}
        </>
      )}
    </div>
  );
}
