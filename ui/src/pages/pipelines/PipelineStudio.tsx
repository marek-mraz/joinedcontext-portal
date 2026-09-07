import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { endpointUrl } from "../../components/endpoints/links";
import { Alert, Button, Field, Input, Select } from "../../components/ui";
import { parseModel } from "../models/linkml";
import { entityTypesOf, pickReadEndpoint, spaceOf } from "../spaces/SpaceInside";
import type { PipelineForm } from "./PipelineEditor";

/** How many rows one sample shows: enough to tick a handful, small enough to read. */
export const SAMPLE_LIMIT = 20;
/** A page read once an hour when the author has not said otherwise: 3600 s is `scheduled`. */
const DEFAULT_PERIOD = "1h";

export type SourceKind = "none" | "datasource" | "space";
export type Aggregate = "sum" | "average" | "count";
export const AGGREGATES: Aggregate[] = ["sum", "average", "count"];

/** One row of a keyValues sample: the id plus whatever attributes the entity carries. */
export type SampleRow = Record<string, unknown> & { id: string };

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
  const source = model?.spec.linkml ?? model?.spec.source;
  if (!type || typeof source !== "string" || !source.includes("\n")) {
    return [];
  }
  const cls = parseModel(source).classes.find((c) => c.name === type);
  return (cls?.slots ?? []).filter((slot) => !["id", "type", "@context"].includes(slot));
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

/** A keyValues page read through the endpoint with the signed-in session, like the look-inside. */
export async function fetchSample(
  slug: string,
  query: { type: string; q?: string; attrs?: string[] },
): Promise<SampleRow[]> {
  const params = new URLSearchParams({
    type: query.type,
    limit: String(SAMPLE_LIMIT),
    options: "keyValues",
  });
  if (query.q?.trim()) {
    params.set("q", query.q.trim());
  }
  if (query.attrs && query.attrs.length > 0) {
    params.set("attrs", query.attrs.join(","));
  }
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?${params.toString()}`), {
      headers: { Accept: "application/ld+json" },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  const body: unknown = await response.json();
  return (Array.isArray(body) ? body : []).filter(
    (item): item is SampleRow =>
      typeof item === "object" && item !== null && typeof (item as SampleRow).id === "string",
  );
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
}: PipelineStudioProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const [sample, setSample] = useState<SampleRow[] | null>(null);
  const [sampleError, setSampleError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [aggregateAttribute, setAggregateAttribute] = useState("");
  // The kind and the space are the author's choice until the form carries them: a chosen kind
  // with nothing picked yet, or a space with no endpoint, is not in the manifest at all.
  const [kindChoice, setKindChoice] = useState<SourceKind>(() => sourceKindOf(draft));
  const [spaceChoice, setSpaceChoice] = useState<string | undefined>(undefined);

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
  const attributes = attributesOf(model, type);
  if (attributes.length === 0) {
    // No inline model: the sample's own keys are the attributes there are.
    for (const row of sample ?? []) {
      for (const key of Object.keys(row)) {
        if (!["id", "type", "@context"].includes(key) && !attributes.includes(key)) {
          attributes.push(key);
        }
      }
    }
  }
  const slug = typeof endpoint?.spec.slug === "string" ? (endpoint.spec.slug as string) : undefined;
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
    update((form) => ({ ...form, source: name ? { dataSourceRef: name } : {} }));
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
    update((form) => ({ ...form, source: { ...form.source, endpointRef: name, query: undefined } }));
  }

  function chooseType(next: string) {
    setSample(null);
    setQuery({ type: next || undefined, attrs: undefined, ids: undefined });
  }

  function toggleAttribute(attribute: string, on: boolean) {
    const current = (draft?.source?.query?.attrs as string[] | undefined) ?? [];
    const next = on ? [...current, attribute] : current.filter((a) => a !== attribute);
    setQuery({ attrs: next.length > 0 ? next : undefined });
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
      setSample(
        await fetchSample(slug, {
          type,
          q: draft?.source?.query?.q as string | undefined,
          attrs: draft?.source?.query?.attrs as string[] | undefined,
        }),
      );
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
        bloblang: aggregateBloblang(aggregate, { type, attribute, space, outputType }),
      },
      output: { type: outputType, mode: form.output?.mode ?? "upsert" },
    }));
  }

  const sectionClass = "flex flex-col gap-3 rounded-md border border-border bg-surface-subtle p-3";

  return (
    <div className="flex flex-col gap-3" data-testid="pipeline-studio">
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
          <div className="grid gap-3 sm:grid-cols-2">
            <Field id="studio-type" label={t("pipelines.field.queryType")}>
              {types.length > 0 ? (
                <Select id="studio-type" value={type ?? ""} onChange={(event) => chooseType(event.target.value)}>
                  <option value="">—</option>
                  {types.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </Select>
              ) : (
                <Input
                  id="studio-type"
                  value={type ?? ""}
                  placeholder={t("pipelines.studio.typePlaceholder")}
                  onChange={(event) => chooseType(event.target.value)}
                />
              )}
            </Field>
            <Field id="studio-q" label={t("pipelines.field.q")} description={t("pipelines.studio.qHint")}>
              <Input
                id="studio-q"
                value={(draft?.source?.query?.q as string | undefined) ?? ""}
                onChange={(event) => setQuery({ q: event.target.value || undefined })}
              />
            </Field>
          </div>
          {attributes.length > 0 ? (
            <fieldset className="flex flex-wrap gap-2">
              <legend className="mb-1 text-caption text-fg-muted">{t("pipelines.field.attrs")}</legend>
              {attributes.map((attribute) => {
                const on = ((draft?.source?.query?.attrs as string[] | undefined) ?? []).includes(attribute);
                return (
                  <label key={attribute} className="inline-flex items-center gap-1 font-mono text-caption">
                    <input
                      type="checkbox"
                      checked={on}
                      onChange={(event) => toggleAttribute(attribute, event.target.checked)}
                    />
                    {attribute}
                  </label>
                );
              })}
            </fieldset>
          ) : null}
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
              {slug ? t("pipelines.studio.sampleHint", { count: SAMPLE_LIMIT }) : t("pipelines.studio.noEndpoint")}
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
                        <th key={attribute} scope="col" className="px-2 py-1 font-mono font-medium">
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
    </div>
  );
}
