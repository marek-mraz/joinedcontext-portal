import { useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { originTransport, parseGridConfig, sourceFor } from "@joinedcontext/sdk";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized, refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { ActivityFeed } from "../../components/ActivityFeed";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import {
  catalogueUrl,
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
} from "../../components/endpoints/links";
import { SharedWithBadge } from "../../components/endpoints/sharing";
import { PortalEntityGrid } from "../../components/entities/PortalEntityGrid";
import {
  Badge,
  Button,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

const SPACE_LABEL = "joinedcontext.com/space";
const RESULTS_COUNT_HEADER = "NGSILD-Results-Count";
const SAMPLE_LIMIT = 3;

/** The space a manifest belongs to: `spec.contextSpaceRef` first, the space label second. */
export function spaceOf(manifest: Manifest): string | undefined {
  return (
    (refName(manifest.spec.contextSpaceRef) || undefined) ??
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
    <TableRow>
      <TableCell className="font-mono">{type}</TableCell>
      <TableCell align="right" className="font-mono">{count}</TableCell>
      <TableCell>
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
      </TableCell>
    </TableRow>
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

/**
 * The whole space in the grid (T-1434; SP-04, SP-06): the space surface answers by the caller's own
 * grants, so this is what this person may read of the space, not one endpoint's view of it.
 *
 * A read is tried for the chosen type before the grid is built, because the surface answers a
 * caller with no grant with a 404 that says nothing about whether the space exists (SP-06) — and
 * then the page says where this person can see it instead. View only: a space surface publishes no
 * grant document (only `/api/endpoint/{slug}/access` does), and a cell that takes a value the
 * gateway will refuse loses the person's typing (UI-44).
 */
function SpaceData({
  project,
  space,
  types,
  endpoints,
}: {
  project: string;
  space: string;
  types: string[];
  endpoints: Manifest[];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [chosen, setChosen] = useState("");
  const type = types.includes(chosen) ? chosen : (types[0] ?? "");
  const source = useMemo(
    () => sourceFor({ kind: "space", space }, originTransport(), i18n.language),
    [space, i18n.language],
  );

  const probe = useQuery({
    queryKey: ["space-surface", space, type],
    enabled: type !== "",
    retry: false,
    queryFn: async () => {
      await source.query({ type }, { offset: 0, limit: 1 });
      return true;
    },
  });

  const config = useMemo(() => {
    if (type === "") {
      return null;
    }
    return (
      parseGridConfig({
        source: { kind: "space", space },
        type,
        pageSize: 25,
        mode: "view",
        history: { enabled: true },
      }).config ?? null
    );
  }, [space, type]);

  if (types.length === 0) {
    return <p className="text-sm text-surface-fg/70">{t("spaces.inside.dataNoTypes")}</p>;
  }
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-surface-fg/70">{t("spaces.inside.dataLead")}</p>
      <label className="flex w-fit items-center gap-2 text-sm">
        {t("spaces.inside.dataType")}
        <select
          aria-label={t("spaces.inside.dataType")}
          className="focus-ring rounded-md border border-border bg-surface px-2 py-1 text-body text-fg"
          value={type}
          onChange={(event) => setChosen(event.target.value)}
        >
          {types.map((each) => (
            <option key={each} value={each}>
              {each}
            </option>
          ))}
        </select>
      </label>

      {probe.isPending ? <p role="status">{t("app.loading")}</p> : null}
      {probe.isError ? (
        <div className="text-sm text-surface-fg/70">
          <p>{t("spaces.inside.dataThroughEndpoints")}</p>
          <ul className="mt-1 flex flex-wrap gap-2">
            {endpoints.map((endpoint) => {
              const slug = (endpoint.spec as { slug?: string }).slug ?? "";
              return (
                <li key={endpoint.metadata.name}>
                  {slug ? (
                    <EndpointLink href={endpointUrl(slug, "")}>{endpoint.metadata.name}</EndpointLink>
                  ) : (
                    <Badge mono>{endpoint.metadata.name}</Badge>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
      {probe.isSuccess && config ? (
        <PortalEntityGrid
          key={`${space}-${type}`}
          project={project}
          config={config}
          source={source}
          empty={<p className="text-sm text-surface-fg/70">{t("spaces.inside.dataEmpty")}</p>}
        />
      ) : null}
    </div>
  );
}

/**
 * What one endpoint answers, folded into one line (T-2280, UI-26, EP-51).
 *
 * Every representation on its own line made two endpoints fill the screen, and a column that tall
 * cannot be read down — which is the only reason to put audience and state in a table at all. The
 * summary names the first two and counts the rest; opening it shows each representation where it can
 * be clicked, and the documents that belong to the endpoint beside them. `<details>` rather than a
 * menu of our own, because the browser already gives it a keyboard, a role and a state a screen
 * reader announces.
 */
function Representations({
  slug,
  representations,
}: {
  slug: string;
  representations: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const named = representations.slice(0, 2);
  const rest = representations.length - named.length;
  const link = (rep: string) =>
    slug && REPRESENTATION_PATHS[rep] ? (
      <EndpointLink href={endpointUrl(slug, REPRESENTATION_PATHS[rep])}>{rep}</EndpointLink>
    ) : (
      <Badge mono>{rep}</Badge>
    );

  if (representations.length === 0 && !slug) {
    return <span className="text-sm text-surface-fg/60">{t("endpoints.field.noRepresentations")}</span>;
  }

  return (
    <details className="group">
      <summary className="focus-ring cursor-pointer list-none text-sm">
        <span className="font-mono">{named.join(", ")}</span>
        {rest > 0 ? (
          <span className="ml-1 text-surface-fg/70">{t("endpoints.field.more", { count: rest })}</span>
        ) : null}
      </summary>
      <ul className="mt-1 flex flex-wrap gap-1">
        {representations.map((rep) => (
          <li key={rep}>{link(rep)}</li>
        ))}
      </ul>
      {slug ? (
        <ul className="mt-1 flex flex-wrap gap-1">
          {ENDPOINT_LINKS.map((entry) => (
            <li key={entry.key}>
              <EndpointLink href={endpointUrl(slug, entry.path)}>
                {t(`endpoints.link.${entry.key}`)}
              </EndpointLink>
            </li>
          ))}
        </ul>
      ) : null}
    </details>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }): JSX.Element {
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
        <Button
          size="sm"
          onClick={() => {
            void space.refetch();
          }}
          className="mt-2"
        >
          {t("app.error.retry")}
        </Button>
      </div>
    );
  }

  const manifest = space.data as Manifest;
  const dataModelRef = refName(manifest.spec.dataModelRef);
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
          <PageHeader
            title={localized(manifest.metadata.title, locale, manifest.metadata.name)}
            description={<span className="font-mono">{manifest.metadata.name}</span>}
          />
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
            <Table caption={t("spaces.inside.types")}>
              <TableHead>
                <TableHeaderCell>{t("spaces.inside.type")}</TableHeaderCell>
                <TableHeaderCell align="right">{t("spaces.inside.count")}</TableHeaderCell>
                <TableHeaderCell>{t("spaces.inside.samples")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {types.map((type) => (
                  <TypeRow key={type} slug={slug} type={type} />
                ))}
              </TableBody>
            </Table>
          </>
        )}
      </Section>

      <Section title={t("spaces.inside.data")}>
        <SpaceData project={project} space={name} types={types} endpoints={spaceEndpoints} />
      </Section>

      <Section title={t("endpoints.title")}>
        {spaceEndpoints.length === 0 ? (
          <p className="text-sm text-surface-fg/70">
            {endpoints.isPending ? t("app.loading") : t("spaces.inside.noEndpoints")}
          </p>
        ) : (
          <Table caption={t("endpoints.title")}>
            <TableHead>
              <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
              <TableHeaderCell>{t("endpoints.field.audience")}</TableHeaderCell>
              <TableHeaderCell>{t("endpoints.field.representations")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.catalogue")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {spaceEndpoints.map((endpoint) => {
                const spec = endpoint.spec as {
                  slug?: string;
                  audience?: string;
                  enabledRepresentations?: string[];
                  policyRef?: string;
                };
                const endpointSlug = spec.slug ?? "";
                return (
                  <TableRow key={endpoint.metadata.name}>
                    <TableCell>
                      <div className="font-medium">
                        {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
                      </div>
                      <div className="font-mono text-xs text-surface-fg/60">
                        {endpoint.metadata.title ? endpoint.metadata.name : null}
                        {spec.policyRef
                          ? `${endpoint.metadata.title ? " · " : ""}${refName(spec.policyRef)}`
                          : ""}
                      </div>
                    </TableCell>
                    <TableCell>
                      <SharedWithBadge endpoint={endpoint} />
                    </TableCell>
                    <TableCell>
                      <Representations
                        slug={endpointSlug}
                        representations={spec.enabledRepresentations ?? []}
                      />
                    </TableCell>
                    <TableCell>
                      <EndpointLink href={catalogueUrl(endpoint.metadata.name)}>
                        {t("spaces.inside.catalogueLink")}
                      </EndpointLink>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </Section>

      <Section title={t("spaces.inside.policies")}>
        {spacePolicies.length === 0 ? (
          <p className="text-sm text-surface-fg/70">
            {policies.isPending ? t("app.loading") : t("spaces.inside.noPolicies")}
          </p>
        ) : (
          <Table caption={t("spaces.inside.policies")}>
            <TableHead>
              <TableHeaderCell>{t("spaces.field.name")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.assignee")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.operations")}</TableHeaderCell>
              <TableHeaderCell>{t("spaces.inside.type")}</TableHeaderCell>
            </TableHead>
            <TableBody>
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
                  <TableRow key={policy.metadata.name}>
                    <TableCell>
                      <div className="font-medium">
                        {localized(policy.metadata.title, locale, policy.metadata.name)}
                      </div>
                      {policy.metadata.title ? (
                        <div className="font-mono text-xs text-surface-fg/60">
                          {policy.metadata.name}
                        </div>
                      ) : null}
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      {spec.assignee
                        ? `${spec.assignee.kind ?? ""}${spec.assignee.kind ? ":" : ""}${spec.assignee.id ?? ""}`
                        : "—"}
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      {(spec.operations ?? []).join(", ") || "—"}
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      {policyTypes.join(", ") || "—"}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </Section>

      <Section title={t("activity.panelTitle")}>
        <ActivityFeed project={project} compact fixed={{ space: name }} limit={10} />
      </Section>
    </div>
  );
}
