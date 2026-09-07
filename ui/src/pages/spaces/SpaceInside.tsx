import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import {
  catalogueUrl,
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
} from "../../components/endpoints/links";
import { SharedWithBadge } from "../../components/endpoints/sharing";
import { parseModel } from "../models/linkml";

const SPACE_LABEL = "joinedcontext.com/space";
const RESULTS_COUNT_HEADER = "NGSILD-Results-Count";
const SAMPLE_LIMIT = 3;

/** A `contextSpaceRef` is a bare name or a `{ kind, name }` reference; both name the space. */
function referenceName(reference: unknown): string | undefined {
  if (typeof reference === "string") {
    return reference;
  }
  if (typeof reference === "object" && reference !== null && "name" in reference) {
    const name = (reference as { name?: unknown }).name;
    return typeof name === "string" ? name : undefined;
  }
  return undefined;
}

/** The space a manifest belongs to: `spec.contextSpaceRef` first, the space label second. */
export function spaceOf(manifest: Manifest): string | undefined {
  return (
    referenceName(manifest.spec.contextSpaceRef) ??
    (manifest.metadata.labels as Record<string, string> | null | undefined)?.[SPACE_LABEL]
  );
}

/**
 * The endpoint the portal reads a space through: one without a policy narrows nothing, so it
 * shows everything the space holds; failing that the first public one is at least readable.
 */
export function pickReadEndpoint(endpoints: Manifest[]): Manifest | undefined {
  return (
    endpoints.find((endpoint) => endpoint.spec.policyRef === undefined) ??
    endpoints.find((endpoint) => endpoint.spec.audience === "public")
  );
}

/**
 * The entity types a DataModel defines: `spec.classes` when the manifest lists them, else the
 * class names of an inline LinkML source (`spec.linkml` carrying a document, not a path).
 */
export function entityTypesOf(model: Manifest): string[] {
  const classes = model.spec.classes;
  if (Array.isArray(classes)) {
    return classes.filter((c): c is string => typeof c === "string");
  }
  const source = model.spec.linkml ?? model.spec.source;
  if (typeof source === "string" && source.includes("\n")) {
    return parseModel(source).classes.map((c) => c.name);
  }
  return [];
}

/** The `NGSILD-Results-Count` header as a number; `undefined` when absent or not a number. */
export function parseResultsCount(headers: Headers): number | undefined {
  const raw = headers.get(RESULTS_COUNT_HEADER);
  if (raw === null) {
    return undefined;
  }
  const count = Number.parseInt(raw.trim(), 10);
  return Number.isNaN(count) || count < 0 ? undefined : count;
}

type KeyValues = Record<string, unknown> & { id: string };

interface TypeInside {
  count?: number;
  samples: KeyValues[];
}

async function gatewayGet(slug: string, query: URLSearchParams): Promise<Response> {
  // A `Request` rather than a URL string, as `api/client.ts` sends: same origin, default mode.
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?${query.toString()}`), {
      headers: { Accept: "application/ld+json" },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  return response;
}

/** What the gateway holds under one type: the live count and a few keyValues samples. */
async function fetchTypeInside(slug: string, type: string): Promise<TypeInside> {
  const counted = await gatewayGet(
    slug,
    new URLSearchParams({ type, limit: "1", count: "true" }),
  );
  const count = parseResultsCount(counted.headers);

  const sampled = await gatewayGet(
    slug,
    new URLSearchParams({ type, limit: String(SAMPLE_LIMIT), options: "keyValues" }),
  );
  const body: unknown = await sampled.json();
  const samples = (Array.isArray(body) ? body : [])
    .filter(
      (item): item is KeyValues =>
        typeof item === "object" && item !== null && typeof (item as KeyValues).id === "string",
    )
    .slice(0, SAMPLE_LIMIT);
  return { count, samples };
}

/** One entity in keyValues form as a single line: its id, then a few attributes. */
function summarize(entity: KeyValues): string {
  const attributes = Object.entries(entity)
    .filter(([key]) => key !== "id" && key !== "type" && key !== "@context")
    .slice(0, 4)
    .map(([key, value]) => {
      const text = typeof value === "string" ? value : JSON.stringify(value);
      return `${key}=${text.length > 40 ? `${text.slice(0, 39)}…` : text}`;
    });
  return attributes.join("  ");
}

function TypeRow({ slug, type }: { slug?: string; type: string }): JSX.Element {
  const { t } = useTranslation();
  const inside = useQuery({
    queryKey: ["gateway", slug ?? "", "inside", type],
    enabled: slug !== undefined,
    retry: false,
    queryFn: () => fetchTypeInside(slug ?? "", type),
  });

  let count: JSX.Element | string;
  if (slug === undefined) {
    count = <span className="text-surface-fg/40">—</span>;
  } else if (inside.isPending) {
    count = <span className="text-surface-fg/60">{t("app.loading")}</span>;
  } else if (inside.isError) {
    const status = inside.error instanceof ApiError ? inside.error.status : undefined;
    count = (
      <span className="text-surface-fg/70">
        {status === 403 || status === 404 || status === 401
          ? t("spaces.inside.notReadable")
          : t("app.error.generic")}
      </span>
    );
  } else {
    count = inside.data.count === undefined ? "—" : String(inside.data.count);
  }

  return (
    <tr className="hover:bg-surface-subtle/50">
      <td className="px-4 py-3 font-mono">{type}</td>
      <td className="px-4 py-3 text-right font-mono">{count}</td>
      <td className="px-4 py-3">
        {inside.isSuccess && inside.data.samples.length > 0 ? (
          <ul className="space-y-1">
            {inside.data.samples.map((entity) => (
              <li key={entity.id} className="font-mono text-xs">
                <span className="text-surface-fg">{entity.id}</span>
                {summarize(entity) ? (
                  <span className="ml-2 text-surface-fg/60">{summarize(entity)}</span>
                ) : null}
              </li>
            ))}
          </ul>
        ) : inside.isSuccess ? (
          <span className="text-xs text-surface-fg/60">{t("spaces.inside.noEntities")}</span>
        ) : null}
      </td>
    </tr>
  );
}

function useProjectList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
}

function Section({ title, children }: { title: string; children: React.ReactNode }): JSX.Element {
  return (
    <section className="space-y-2">
      <h2 className="text-base font-semibold">{title}</h2>
      {children}
    </section>
  );
}

/** What a Context Space holds: its entity types with live counts, its endpoints and policies. */
export function SpaceInside({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const space = useQuery({
    queryKey: queryKeys.resource(project, "spaces", name),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "spaces", name } },
        }),
      ),
  });
  const models = useProjectList(project, "datamodels");
  const endpoints = useProjectList(project, "endpoints");
  const policies = useProjectList(project, "policies");

  if (space.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (space.isError) {
    const message =
      space.error instanceof ApiError
        ? (space.error.problem?.detail ?? space.error.message)
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <button
          type="button"
          onClick={() => {
            void space.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  const manifest = space.data as Manifest;
  const dataModelRef = manifest.spec.dataModelRef;
  const model = asManifests(models.data?.items ?? []).find(
    (m) => m.metadata.name === dataModelRef,
  );
  const types = model ? entityTypesOf(model) : [];
  const spaceEndpoints = asManifests(endpoints.data?.items ?? []).filter(
    (endpoint) => spaceOf(endpoint) === name,
  );
  const readEndpoint = pickReadEndpoint(spaceEndpoints);
  const slug =
    typeof readEndpoint?.spec.slug === "string" ? (readEndpoint.spec.slug as string) : undefined;
  const spacePolicies = asManifests(policies.data?.items ?? []).filter(
    (policy) => spaceOf(policy) === name,
  );

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <Link
            to="/projects/$project/$plural"
            params={{ project, plural: "spaces" }}
            className="text-sm text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("spaces.inside.back")}
          </Link>
          <h1 className="text-xl font-bold">
            {localized(manifest.metadata.title, locale, manifest.metadata.name)}
          </h1>
          <div className="font-mono text-xs text-surface-fg/60">{manifest.metadata.name}</div>
        </div>
        <LifecycleBadge kind="phase" value={manifest.status?.phase} />
      </div>

      <Section title={t("spaces.inside.types")}>
        {model === undefined ? (
          <p className="text-sm text-surface-fg/70">
            {models.isPending ? t("app.loading") : t("spaces.inside.noModel")}
          </p>
        ) : types.length === 0 ? (
          <p className="text-sm text-surface-fg/70">
            {t("spaces.inside.noTypes", { model: model.metadata.name })}
          </p>
        ) : (
          <>
            <p className="text-sm text-surface-fg/70">
              {t("spaces.field.dataModel")}: <span className="font-mono">{model.metadata.name}</span>
              {readEndpoint ? (
                <>
                  {" · "}
                  {t("spaces.inside.readThrough", { endpoint: readEndpoint.metadata.name })}
                </>
              ) : (
                <>
                  {" · "}
                  {endpoints.isPending ? t("app.loading") : t("spaces.inside.noEndpoint")}
                </>
              )}
            </p>
            <div className="overflow-x-auto rounded border border-border">
              <table className="w-full border-collapse text-left text-sm">
                <caption className="sr-only">{t("spaces.inside.types")}</caption>
                <thead>
                  <tr className="border-b border-border bg-surface-subtle">
                    <th scope="col" className="px-4 py-2 font-medium">
                      {t("spaces.inside.type")}
                    </th>
                    <th scope="col" className="px-4 py-2 font-medium text-right">
                      {t("spaces.inside.count")}
                    </th>
                    <th scope="col" className="px-4 py-2 font-medium">
                      {t("spaces.inside.samples")}
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border">
                  {types.map((type) => (
                    <TypeRow key={type} slug={slug} type={type} />
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </Section>

      <Section title={t("endpoints.title")}>
        {spaceEndpoints.length === 0 ? (
          <p className="text-sm text-surface-fg/70">
            {endpoints.isPending ? t("app.loading") : t("spaces.inside.noEndpoints")}
          </p>
        ) : (
          <div className="overflow-x-auto rounded border border-border">
            <table className="w-full border-collapse text-left text-sm">
              <caption className="sr-only">{t("endpoints.title")}</caption>
              <thead>
                <tr className="border-b border-border bg-surface-subtle">
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("endpoints.field.name")}
                  </th>
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("endpoints.field.audience")}
                  </th>
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("endpoints.field.representations")}
                  </th>
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("spaces.inside.catalogue")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {spaceEndpoints.map((endpoint) => {
                  const spec = endpoint.spec as {
                    slug?: string;
                    audience?: string;
                    enabledRepresentations?: string[];
                    policyRef?: string;
                  };
                  const endpointSlug = spec.slug ?? "";
                  return (
                    <tr key={endpoint.metadata.name} className="hover:bg-surface-subtle/50">
                      <td className="px-4 py-3">
                        <div className="font-medium">
                          {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
                        </div>
                        <div className="font-mono text-xs text-surface-fg/60">
                          {endpoint.metadata.title ? endpoint.metadata.name : null}
                          {spec.policyRef
                            ? `${endpoint.metadata.title ? " · " : ""}${spec.policyRef}`
                            : ""}
                        </div>
                      </td>
                      <td className="px-4 py-3">
                        <SharedWithBadge endpoint={endpoint} />
                      </td>
                      <td className="px-4 py-3">
                        <ul className="flex flex-wrap gap-1">
                          {(spec.enabledRepresentations ?? []).map((rep) => (
                            <li key={rep}>
                              {endpointSlug && REPRESENTATION_PATHS[rep] ? (
                                <EndpointLink
                                  href={endpointUrl(endpointSlug, REPRESENTATION_PATHS[rep])}
                                >
                                  {rep}
                                </EndpointLink>
                              ) : (
                                <span className="inline-flex items-center rounded border border-border px-2 py-0.5 font-mono text-xs">
                                  {rep}
                                </span>
                              )}
                            </li>
                          ))}
                        </ul>
                        {endpointSlug ? (
                          <ul className="mt-1 flex flex-wrap gap-1">
                            {ENDPOINT_LINKS.map((link) => (
                              <li key={link.key}>
                                <EndpointLink href={endpointUrl(endpointSlug, link.path)}>
                                  {t(`endpoints.link.${link.key}`)}
                                </EndpointLink>
                              </li>
                            ))}
                          </ul>
                        ) : null}
                      </td>
                      <td className="px-4 py-3">
                        <EndpointLink href={catalogueUrl(endpoint.metadata.name)}>
                          {t("spaces.inside.catalogueLink")}
                        </EndpointLink>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Section>

      <Section title={t("spaces.inside.policies")}>
        {spacePolicies.length === 0 ? (
          <p className="text-sm text-surface-fg/70">
            {policies.isPending ? t("app.loading") : t("spaces.inside.noPolicies")}
          </p>
        ) : (
          <div className="overflow-x-auto rounded border border-border">
            <table className="w-full border-collapse text-left text-sm">
              <caption className="sr-only">{t("spaces.inside.policies")}</caption>
              <thead>
                <tr className="border-b border-border bg-surface-subtle">
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("spaces.field.name")}
                  </th>
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("spaces.inside.assignee")}
                  </th>
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("spaces.inside.operations")}
                  </th>
                  <th scope="col" className="px-4 py-2 font-medium">
                    {t("spaces.inside.type")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {spacePolicies.map((policy) => {
                  const spec = policy.spec as {
                    assignee?: { kind?: string; id?: string };
                    operations?: string[];
                    information?: Array<{ entities?: Array<{ type?: string }> }>;
                  };
                  const policyTypes = (spec.information ?? [])
                    .flatMap((info) => info.entities ?? [])
                    .map((entity) => entity.type)
                    .filter((type): type is string => typeof type === "string");
                  return (
                    <tr key={policy.metadata.name} className="hover:bg-surface-subtle/50">
                      <td className="px-4 py-3">
                        <div className="font-medium">
                          {localized(policy.metadata.title, locale, policy.metadata.name)}
                        </div>
                        {policy.metadata.title ? (
                          <div className="font-mono text-xs text-surface-fg/60">
                            {policy.metadata.name}
                          </div>
                        ) : null}
                      </td>
                      <td className="px-4 py-3 font-mono text-xs">
                        {spec.assignee
                          ? `${spec.assignee.kind ?? ""}${spec.assignee.kind ? ":" : ""}${spec.assignee.id ?? ""}`
                          : "—"}
                      </td>
                      <td className="px-4 py-3 font-mono text-xs">
                        {(spec.operations ?? []).join(", ") || "—"}
                      </td>
                      <td className="px-4 py-3 font-mono text-xs">
                        {policyTypes.join(", ") || "—"}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Section>
    </div>
  );
}
