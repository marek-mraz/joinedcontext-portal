import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, api, queryKeys, unwrap, whilePending } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import type { Manifest } from "../../api/manifest";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { SaveAsResourceAction } from "../../components/SaveAsDialog";
import { WorkOnCopyAction } from "../../components/WorkOnCopyDialog";
import { ExportButton } from "../../components/export/ExportButton";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { RowActions } from "../../components/ui/RowActions";
import type { RowAction } from "../../components/ui/RowActions";
import { usePermissions } from "../../api/permissions";
import {
  ENDPOINT_LINKS,
  EndpointLink,
  REPRESENTATION_PATHS,
  catalogueUrl,
  endpointUrl,
} from "../../components/endpoints/links";
import { spaceOf } from "../../components/endpoints/sharing";
import { CopyUrlButton } from "../../routes/EndpointsPage";
import { Alert, Badge, Button, Field, Input, PageHeader, Select, SourceLink } from "../../components/ui";
import { andQ, areaQuery, queryFromFilters, ringOfBounds } from "@joinedcontext/sdk";
import type { FilterOp } from "@joinedcontext/sdk";

/**
 * One endpoint, the whole window (T-2281, EP-51, UI-26, UI-61).
 *
 * The endpoints table had a cramped cell per aspect and every action beside it, so the settings that
 * decide what a city publishes were read three words at a time. This page is the endpoint's own: what
 * it is, what it answers, what it filters out before anybody sees a thing, who may call it, and what
 * it limits — each section labelled, in the order a person asks the questions.
 *
 * What is NOT here, and why: `EndpointSpec` is `deny_unknown_fields`, so the page shows the fields the
 * manifest actually has. **Filtering is not one of them** — a `q`, a `scopeQ` or an area lives on the
 * named `ModelProjection` the endpoint references (`spec.projectionRef`, MP-01), which is why this page
 * reads the filter from there and says so. There is likewise no `paused`/`retiring` field on an
 * endpoint: `status.phase` is the platform's own and never written from a form, so this page paints no
 * pause button that nothing behind it could honour (T-2286 holds the contract question).
 */
export function EndpointPage({
  project,
  name,
}: {
  project: string;
  name: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const navigate = useNavigate();
  const permissions = usePermissions(project);
  // An action a role cannot take stays in the menu with its reason; UI-44 does not stop at a menu's
  // edge (T-2279).
  const denied = (verb: "propose" | "delete") =>
    permissions.can("Endpoint", verb) ? undefined : t("permissions.denied", { verb, kind: "Endpoint" });
  const [openAction, setOpenAction] = useState<"saveAs" | "workOnCopy" | "delete" | null>(null);

  const endpoint = useQuery({
    queryKey: queryKeys.resource(project, "endpoints", name),
    retry: false,
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "endpoints", name } },
        }),
      ),
  });

  const projections = useQuery({
    queryKey: queryKeys.list(project, "projections"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "projections" } },
        }),
      ),
  });

  if (endpoint.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (endpoint.isError) {
    const message =
      endpoint.error instanceof ApiError
        ? (endpoint.error.problem?.detail ?? endpoint.error.message)
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <Button
          size="sm"
          className="mt-2"
          onClick={() => {
            void endpoint.refetch();
          }}
        >
          {t("app.error.retry")}
        </Button>
      </div>
    );
  }

  const manifest = endpoint.data as Manifest;
  const spec = manifest.spec as EndpointSpec;
  const slug = spec.slug ?? "";
  const space = spaceOf(manifest);
  const projectionName = refName(spec.projectionRef);
  const projection = projectionName
    ? asManifests(projections.data?.items ?? []).find((p) => p.metadata.name === projectionName)
    : undefined;
  const target = { project, kind: "Endpoint", plural: "endpoints", name } as const;

  // The actions that open a dialog are rendered beside the menu, never inside it: the menu unmounts
  // its content when it closes and would take an open dialog with it (T-2279).
  const actions: RowAction[] = [
    ...(slug
      ? [
          {
            key: "copy",
            label: t("endpoints.copyUrl"),
            onSelect: () => {
              void navigator.clipboard?.writeText(endpointUrl(slug, "")).catch(() => undefined);
            },
          },
        ]
      : []),
    {
      key: "saveAs",
      label: t("saveAs.button"),
      onSelect: () => setOpenAction("saveAs"),
      disabledReason: denied("propose"),
    },
    {
      key: "workOnCopy",
      label: t("workspaces.open.action"),
      onSelect: () => setOpenAction("workOnCopy"),
    },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setOpenAction("delete"),
      disabledReason: denied("delete"),
    },
  ];

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <Link
            to="/projects/$project/$plural"
            params={{ project, plural: "endpoints" }}
            className="text-sm text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("endpoints.page.back")}
          </Link>
          <PageHeader
            title={localized(manifest.metadata.title, locale, name)}
            description={<span className="font-mono">{name}</span>}
          />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <LifecycleBadge kind="phase" value={manifest.status?.phase} />
          {manifest.status?.sourceUrl ? (
            <SourceLink href={manifest.status.sourceUrl} label={t("spaces.field.source")} />
          ) : null}
        </div>
      </div>

      {/* Every change here is proposed and reviewed like any other, so the page says so before the
          first control rather than after a person has pressed one (PF-57). */}
      <p className="text-sm text-fg-muted">{t("endpoints.page.proposedNotice")}</p>

      <div className="flex flex-wrap items-center gap-2">
        <PermissionGuard project={project} kind="Endpoint" verb="propose">
          <Button
            onClick={() => {
              void navigate({
                to: "/projects/$project/$plural",
                params: { project, plural: "endpoints" },
                search: { edit: name },
              });
            }}
          >
            {t("endpoints.page.change")}
          </Button>
        </PermissionGuard>
        <ExportButton
          project={project}
          target={{ plural: "endpoints", name }}
          label={t("export.action")}
          size="sm"
        />
        <RowActions label={localized(manifest.metadata.title, locale, name)} actions={actions} />
        <SaveAsResourceAction
          target={target}
          trigger={false}
          open={openAction === "saveAs"}
          onOpenChange={(next) => setOpenAction(next ? "saveAs" : null)}
        />
        <WorkOnCopyAction
          project={project}
          scope={{ kind: "resources", items: [{ kind: "Endpoint", name }] }}
          trigger={false}
          open={openAction === "workOnCopy"}
          onOpenChange={(next) => setOpenAction(next ? "workOnCopy" : null)}
        />
        <DeleteResourceAction
          target={target}
          trigger={false}
          open={openAction === "delete"}
          onOpenChange={(next) => setOpenAction(next ? "delete" : null)}
        />
      </div>

      <Section title={t("endpoints.page.whatItIs")}>
        <Facts>
          <Fact label={t("endpoints.field.space")}>
            {space ? (
              <Link
                to="/projects/$project/spaces/$name"
                params={{ project, name: space }}
                className="text-primary underline hover:no-underline"
              >
                {space}
              </Link>
            ) : (
              <Missing />
            )}
          </Fact>
          <Fact label={t("endpoints.field.audience")}>
            <Badge>{t(`endpoints.audience.${spec.audience ?? "project-list"}`, spec.audience ?? "")}</Badge>
            {spec.audience === "public" ? (
              <p className="mt-1 text-caption text-fg-muted">{t("endpoints.publicNotice")}</p>
            ) : null}
          </Fact>
          {spec.audience === "project-list" ? (
            <Fact label={t("endpoints.field.allowedProjects")}>
              {spec.allowedProjects?.length ? (
                <ul className="flex flex-wrap gap-1">
                  {spec.allowedProjects.map((allowed) => (
                    <li key={allowed}>
                      <Badge mono>{allowed}</Badge>
                    </li>
                  ))}
                </ul>
              ) : (
                <Missing />
              )}
            </Fact>
          ) : null}
          <Fact label={t("endpoints.field.publicUrl")}>
            {slug ? (
              <div className="flex flex-wrap items-center gap-2">
                <code className="break-all font-mono text-caption">{endpointUrl(slug, "")}</code>
                {/* The one copy control that confirms it copied; the row's menu item cannot, because
                    the menu closes on select (T-2287). */}
                <CopyUrlButton slug={slug} />
              </div>
            ) : (
              <Missing />
            )}
          </Fact>
        </Facts>
      </Section>

      <Section title={t("endpoints.page.whatItAnswers")}>
        <Facts>
          <Fact label={t("endpoints.field.representations")}>
            <ul className="flex flex-wrap gap-1">
              {(spec.enabledRepresentations ?? []).map((rep) => (
                <li key={rep}>
                  {slug && REPRESENTATION_PATHS[rep] ? (
                    <EndpointLink href={endpointUrl(slug, REPRESENTATION_PATHS[rep])}>{rep}</EndpointLink>
                  ) : (
                    <Badge mono>{rep}</Badge>
                  )}
                </li>
              ))}
            </ul>
          </Fact>
          {slug ? (
            <Fact label={t("endpoints.page.alwaysThere")}>
              <ul className="flex flex-wrap gap-1">
                {ENDPOINT_LINKS.map((link) => (
                  <li key={link.key}>
                    <EndpointLink muted href={endpointUrl(slug, link.path)}>
                      {t(`endpoints.link.${link.key}`)}
                    </EndpointLink>
                  </li>
                ))}
              </ul>
            </Fact>
          ) : null}
          <Fact label={t("endpoints.field.types")}>
            {projection ? (
              <ul className="flex flex-wrap gap-1">
                {classesOf(projection).map((klass) => (
                  <li key={klass}>
                    <Badge mono>{klass}</Badge>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.everyTypeOfTheSpace")}</p>
            )}
          </Fact>
          <Fact label={t("endpoints.field.hiddenAttributes")}>
            {spec.projection?.hiddenAttributes?.length ? (
              <ul className="flex flex-wrap gap-1">
                {spec.projection.hiddenAttributes.map((attr) => (
                  <li key={attr}>
                    <Badge mono tone="warning">
                      {attr}
                    </Badge>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.nothingHidden")}</p>
            )}
          </Fact>
          {spec.viewMappingRef ? (
            <Fact label={t("endpoints.field.viewMapping")}>
              <Badge mono>{refName(spec.viewMappingRef)}</Badge>
              <p className="mt-1 text-caption text-fg-muted">{t("endpoints.page.viewMappingReadOnly")}</p>
            </Fact>
          ) : null}
        </Facts>
      </Section>

      <Section title={t("endpoints.page.filtering")} lead={t("endpoints.page.filteringLead")}>
        {projection ? (
          <>
            <FilterForm
              project={project}
              projection={projection}
              slug={slug}
              live={(manifest.status?.phase ?? "").toLowerCase() === "live"}
            />
            <p className="text-caption text-fg-muted">
              {t("endpoints.page.filterLivesOn")}{" "}
              <Link
                to="/projects/$project/$plural"
                params={{ project, plural: "projections" }}
                className="text-primary underline hover:no-underline"
              >
                {projectionName}
              </Link>
            </p>
          </>
        ) : (
          <p className="text-sm text-fg-muted">{t("endpoints.page.noProjection")}</p>
        )}
      </Section>

      <Section title={t("endpoints.page.whoMayCall")}>
        {spec.policyRef ? (
          <Facts>
            <Fact label={t("endpoints.field.policy")}>
              <code className="break-all font-mono text-caption">{spec.policyRef}</code>
            </Fact>
          </Facts>
        ) : (
          <p className="text-sm text-fg-muted">{t("endpoints.page.noPolicy")}</p>
        )}
        <Link
          to="/projects/$project/$plural"
          params={{ project, plural: "policies" }}
          className="text-caption text-primary underline hover:no-underline"
        >
          {t("endpoints.page.openPolicies")}
        </Link>
      </Section>

      <Section title={t("endpoints.page.traffic")}>
        <Facts>
          <Fact label={t("endpoints.field.rateLimit")}>
            {typeof spec.rateLimits?.requestsPerMinute === "number" ? (
              <span>
                {t("endpoints.page.perMinute", { count: spec.rateLimits.requestsPerMinute })}
                {typeof spec.rateLimits.burst === "number"
                  ? ` · ${t("endpoints.page.burst", { count: spec.rateLimits.burst })}`
                  : ""}
              </span>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.noRateLimit")}</p>
            )}
          </Fact>
          <Fact label={t("endpoints.field.caching")}>
            {typeof spec.caching?.maxAgeSeconds === "number" ? (
              <span>{t("endpoints.page.maxAge", { count: spec.caching.maxAgeSeconds })}</span>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.noCaching")}</p>
            )}
          </Fact>
          <Fact label={t("endpoints.field.fileLimits")}>
            {spec.fileLimits?.maxFileRows || spec.fileLimits?.maxFileBytes ? (
              <ul className="space-y-0.5">
                {spec.fileLimits.maxFileRows ? (
                  <li>{t("endpoints.page.maxRows", { count: spec.fileLimits.maxFileRows })}</li>
                ) : null}
                {spec.fileLimits.maxFileBytes ? (
                  <li>{t("endpoints.page.maxBytes", { count: spec.fileLimits.maxFileBytes })}</li>
                ) : null}
              </ul>
            ) : (
              <p className="text-caption text-fg-muted">{t("endpoints.page.noFileLimits")}</p>
            )}
          </Fact>
        </Facts>
      </Section>

      {spec.publish?.ckan ? (
        <Section title={t("endpoints.page.publication")}>
          <Facts>
            <Fact label={t("endpoints.field.ckanInstance")}>
              <Badge mono>{refName(spec.publish.ckan.instanceRef)}</Badge>
            </Fact>
            {spec.publish.ckan.organization ? (
              <Fact label={t("endpoints.field.ckanOrganization")}>
                <Badge mono>{spec.publish.ckan.organization}</Badge>
              </Fact>
            ) : null}
            <Fact label={t("endpoints.field.catalogue")}>
              <a
                href={catalogueUrl(spec.publish.ckan.name ?? name)}
                target="_blank"
                rel="noreferrer"
                className="text-primary underline hover:no-underline"
              >
                {spec.publish.ckan.name ?? name}
              </a>
            </Fact>
          </Facts>
        </Section>
      ) : null}
    </div>
  );
}

/**
 * The filter of the named projection, editable here (T-2281, the owner's own ask), proposed like any
 * other change.
 *
 * Four conditions is what the manifest holds — `q`, `scopeQ`, `geoQ`, `temporalQ` (MP-01) — and each
 * one only ever narrows what the endpoint publishes; there is no field here that could widen a grant.
 * An empty box removes the condition rather than storing an empty query, so "not narrowed" stays what
 * it says. T-2283 turns these four boxes into rows a person builds without knowing NGSI-LD, with the
 * count of what they match; the boxes are what the manifest can express today.
 */
function FilterForm({
  project,
  projection,
  slug,
  live,
}: {
  project: string;
  projection: Manifest;
  /** The endpoint the counts are read through; both are what it answers itself. */
  slug: string;
  live: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const stored = filterOf(projection);
  const [draft, setDraft] = useState<Record<string, string>>(() =>
    Object.fromEntries(FILTER_KEYS.map((key) => [key, stored[key] ?? ""])),
  );
  const [change, setChange] = useState<Change | null>(null);

  const propose = useMutation({
    mutationFn: async () => {
      const filter = Object.fromEntries(
        FILTER_KEYS.map((key) => [key, draft[key]?.trim() ?? ""]).filter(([, value]) => value !== ""),
      );
      // The status is the platform's to compute and never travels back (MF-04).
      const rest = { ...projection };
      delete (rest as { status?: unknown }).status;
      const body = {
        ...rest,
        spec: {
          ...(projection.spec as Record<string, unknown>),
          ...(Object.keys(filter).length > 0 ? { filter } : {}),
        },
      };
      if (Object.keys(filter).length === 0) {
        delete (body.spec as { filter?: unknown }).filter;
      }
      return proposeChecked(project, "projections", body as { metadata: { name: string } }, false);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "projections") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const faults = Object.fromEntries(
    FILTER_KEYS.map((key) => [key, filterFault(key, draft[key] ?? "")]),
  ) as Record<string, string | undefined>;
  const faulty = Object.values(faults).some(Boolean);
  const untouched = FILTER_KEYS.every((key) => (draft[key]?.trim() ?? "") === (stored[key] ?? ""));
  const failure =
    propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null;

  if (change) {
    return <ChangeNotice change={change} project={project} />;
  }

  return (
    <form
      className="space-y-3"
      onSubmit={(event) => {
        event.preventDefault();
        propose.mutate();
      }}
    >
      {failure ? (
        <Alert tone="danger" role="alert">
          {failure}
        </Alert>
      ) : null}
      <div className="grid gap-3 sm:grid-cols-2">
        {FILTER_KEYS.map((key) => (
          <Field
            key={key}
            id={`filter-${key}`}
            label={t(`endpoints.filter.${key}`)}
            help={t(`endpoints.filter.${key}Help`)}
            errors={faults[key] ? [t(`endpoints.filter.fault.${faults[key] as string}`)] : undefined}
          >
            <Input
              id={`filter-${key}`}
              value={draft[key] ?? ""}
              placeholder={t("endpoints.page.filterEmpty")}
              aria-invalid={faults[key] ? true : undefined}
              onChange={(event) => setDraft({ ...draft, [key]: event.target.value })}
            />
          </Field>
        ))}
      </div>
      <MatchCount slug={slug} type={classesOf(projection)[0]} q={draft.q ?? ""} live={live} />
      <AreaFromBox onSet={(geoQ) => setDraft({ ...draft, geoQ })} />
      <ConditionBuilder
        attributes={slotsOf(projection)}
        onAdd={(term) =>
          setDraft({ ...draft, q: andQ(draft.q, term) ?? "" })
        }
      />
      <PermissionGuard project={project} kind="ModelProjection" verb="propose">
        <Button type="submit" disabled={untouched || faulty || propose.isPending}>
          {t("endpoints.page.filterPropose")}
        </Button>
      </PermissionGuard>
    </form>
  );
}

/** The results count header of a counted NGSI-LD read (CIM 009 6.3.13). */
const RESULTS_COUNT = "NGSILD-Results-Count";

async function countThrough(slug: string, params: URLSearchParams): Promise<number> {
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, `/ngsi-ld/v1/entities?${params.toString()}`), {
      headers: { Accept: "application/ld+json" },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  const raw = response.headers.get(RESULTS_COUNT);
  const count = raw === null ? Number.NaN : Number.parseInt(raw.trim(), 10);
  if (Number.isNaN(count) || count < 0) {
    throw new ApiError(502, "the endpoint answered no results count");
  }
  return count;
}

/**
 * How many entities the filter matches, of how many the endpoint answers (T-2283).
 *
 * A filter that matches nothing has to be visible before it is proposed — with a million entities in a
 * space, nobody can tell by reading a query string. Both counts are read **through this endpoint**, so
 * they are what the endpoint itself answers under the reader's own rights: no privileged path, and a
 * person who may not read the data sees the refusal, not a number they should not have.
 *
 * Only a Live endpoint answers at all, and NGSI-LD refuses a query with no selector, so this needs the
 * projection's own type; without either, it says so instead of showing a zero that would read as an
 * empty space.
 */
function MatchCount({
  slug,
  type,
  q,
  live,
}: {
  slug: string;
  type?: string;
  q: string;
  live: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const asked = q.trim();
  const counted = useQuery({
    queryKey: ["endpoint", slug, "matches", type ?? "", asked],
    enabled: live && slug !== "" && type !== undefined,
    retry: false,
    queryFn: async () => {
      const base = { type: type as string, limit: "1", count: "true" };
      const all = await countThrough(slug, new URLSearchParams(base));
      const matching = asked === "" ? all : await countThrough(slug, new URLSearchParams({ ...base, q: asked }));
      return { all, matching };
    },
  });

  if (!live || type === undefined) {
    return <p className="text-caption text-fg-muted">{t("endpoints.count.unavailable")}</p>;
  }
  if (counted.isPending) {
    return (
      <p role="status" className="text-caption text-fg-muted">
        {t("endpoints.count.counting")}
      </p>
    );
  }
  if (counted.isError) {
    const message =
      counted.error instanceof ApiError
        ? (counted.error.problem?.detail ?? counted.error.message)
        : t("app.error.generic");
    return (
      <p role="status" className="text-caption text-fg-muted">
        {t("endpoints.count.failed", { reason: message })}
      </p>
    );
  }
  const { all, matching } = counted.data;
  return (
    <p role="status" className={matching === 0 ? "text-caption text-danger" : "text-caption text-fg-muted"}>
      {t("endpoints.count.matches", { matching, all })}
      {matching === 0 ? ` ${t("endpoints.count.none")}` : ""}
    </p>
  );
}

/**
 * What is wrong with one condition of the filter, in a sentence, or `undefined` (T-2283, UI-45).
 *
 * The server checks the manifest too, and it is the authority; this only spares a person a round trip on
 * the two mistakes the format makes easy. A scope is a path (`/Helsinki/Kamppi`, CIM 009 4.19), not free
 * text, and a `geoQ` carries `georel`, `geometry` and `coordinates` together or the gateway cannot read
 * any of it.
 */
export function filterFault(key: string, value: string): "scopePath" | "geoParts" | undefined {
  const written = value.trim();
  if (written === "") {
    return undefined;
  }
  if (key === "scopeQ") {
    // A scope path's segments may hold spaces ("/Banska Bystrica/Centrum"); what it may not do is start
    // without a slash or carry an empty segment, which is the mistake free text makes.
    const paths = written
      .split(/[;,|]/)
      .map((part) => part.trim())
      .filter((part) => part !== "");
    return paths.every((path) => /^\/[^/]+(\/[^/]+)*$/.test(path)) ? undefined : "scopePath";
  }
  if (key === "geoQ") {
    const parts = new Set(
      written
        .split(";")
        .map((part) => part.split("=")[0]?.trim())
        .filter((part): part is string => Boolean(part)),
    );
    return ["georel", "geometry", "coordinates"].every((part) => parts.has(part)) ? undefined : "geoParts";
  }
  return undefined;
}

/** `geoQ` as the manifest holds it: the parts of a geo query joined with `;` (MP-01, CIM 009 4.10). */
export function geoQueryString(query: {
  georel: string;
  geometry: string;
  coordinates: string;
  geoproperty: string;
}): string {
  return [
    `georel=${query.georel}`,
    `geometry=${query.geometry}`,
    `coordinates=${query.coordinates}`,
    `geoproperty=${query.geoproperty}`,
  ].join(";");
}

/** What is wrong with a box a person typed, or `undefined`. */
export function boxFault(box: Record<string, string>): "numbers" | "range" | "order" | undefined {
  const values = ["west", "south", "east", "north"].map((key) => Number(box[key]));
  if (values.some((value) => !Number.isFinite(value)) || ["west", "south", "east", "north"].some((k) => (box[k] ?? "").trim() === "")) {
    return "numbers";
  }
  const [west, south, east, north] = values;
  if (Math.abs(west) > 180 || Math.abs(east) > 180 || Math.abs(south) > 90 || Math.abs(north) > 90) {
    return "range";
  }
  return west < east && south < north ? undefined : "order";
}

/**
 * An area from a bounding box (T-2283, UI-72).
 *
 * The owner asked to share only what lies in one area. Typing a geo query by hand means writing
 * `georel`, `geometry`, a JSON ring and `geoproperty` without a mistake; four numbers cannot be written
 * wrong in a way that reaches the manifest, because the ring, the rounding and the length limit of
 * `coordinates` are the SDK's own (`ringOfBounds`, `areaQuery`) — the same code the grid's map uses, so a
 * shape too long for a URL is shortened in one place and said so once (UI-67).
 *
 * Drawing the shape on a map is the grid's job and stays there; this is the part a settings page can do
 * honestly without a map.
 */
function AreaFromBox({ onSet }: { onSet: (geoQ: string) => void }): JSX.Element {
  const { t } = useTranslation();
  const [box, setBox] = useState<Record<string, string>>({ west: "", south: "", east: "", north: "" });
  const [property, setProperty] = useState("location");
  const [shown, setShown] = useState(false);
  const fault = boxFault(box);

  const apply = () => {
    setShown(true);
    if (fault || property.trim() === "") {
      return;
    }
    const query = areaQuery({
      geoproperty: property.trim(),
      geometry: {
        type: "Polygon",
        coordinates: [
          ringOfBounds(Number(box.west), Number(box.south), Number(box.east), Number(box.north)),
        ],
      },
    });
    if (!query) {
      return;
    }
    setShown(false);
    onSet(geoQueryString(query));
  };

  return (
    <div className="space-y-2 rounded-md border border-border p-3">
      <p className="text-caption font-medium text-fg-muted">{t("endpoints.area.title")}</p>
      <div className="flex flex-wrap items-end gap-2">
        {(["west", "south", "east", "north"] as const).map((corner) => (
          <Field key={corner} id={`area-${corner}`} label={t(`endpoints.area.${corner}`)} className="w-28">
            <Input
              id={`area-${corner}`}
              inputMode="decimal"
              value={box[corner] ?? ""}
              aria-invalid={shown && fault ? true : undefined}
              onChange={(event) => setBox({ ...box, [corner]: event.target.value })}
            />
          </Field>
        ))}
        <Field id="area-property" label={t("endpoints.area.property")} className="w-40">
          <Input
            id="area-property"
            value={property}
            onChange={(event) => setProperty(event.target.value)}
          />
        </Field>
        <Button type="button" variant="secondary" onClick={apply}>
          {t("endpoints.area.set")}
        </Button>
      </div>
      {shown && fault ? (
        <p role="alert" className="text-caption text-danger">
          {t(`endpoints.area.fault.${fault}`)}
        </p>
      ) : null}
      <p className="text-caption text-fg-muted">{t("endpoints.area.hint")}</p>
    </div>
  );
}

/** The operators a condition row offers, in the order a person reaches for them. */
const CONDITION_OPS: FilterOp[] = [
  "equals",
  "notEquals",
  "gt",
  "gte",
  "lt",
  "lte",
  "contains",
  "present",
  "empty",
];

/** The attributes a projection publishes, once, in order: what a condition can name. */
export function slotsOf(projection: Manifest): string[] {
  const classes = (projection.spec as { classes?: Array<{ slots?: string[] }> }).classes ?? [];
  const seen = new Set<string>();
  for (const klass of classes) {
    for (const slot of klass.slots ?? []) {
      if (slot) {
        seen.add(slot);
      }
    }
  }
  return [...seen];
}

/**
 * One condition, built instead of typed (T-2283, the owner's `age>30`).
 *
 * It **appends** to the query rather than replacing it: an existing `q` was written by somebody, may
 * use more of NGSI-LD §4.9 than these three boxes can say, and rewriting it from a parse that got it
 * wrong would silently change what a million entities' worth of endpoint publishes. Appending with
 * `;` is what the gateway does with two conditions, and it can only narrow.
 *
 * The term itself is compiled by the SDK's own compiler (`queryFromFilters`), which is what the entity
 * grid's filter row uses: one place quotes a value, escapes a pattern's metacharacters and decides
 * whether a literal is bare or quoted (UI-66, EP-07).
 */
function ConditionBuilder({
  attributes,
  onAdd,
}: {
  attributes: string[];
  onAdd: (term: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [attr, setAttr] = useState(attributes[0] ?? "");
  const [op, setOp] = useState<FilterOp>("equals");
  const [value, setValue] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const needsValue = op !== "present" && op !== "empty";

  const add = () => {
    if (!attr) {
      setInvalid(t("endpoints.condition.needsAttribute"));
      return;
    }
    if (needsValue && value.trim() === "") {
      setInvalid(t("endpoints.condition.needsValue"));
      return;
    }
    const { q } = queryFromFilters(
      [{ key: attr, attr, kind: "text" }],
      { [attr]: { op, value: value.trim() } },
    );
    if (!q) {
      setInvalid(t("endpoints.condition.needsValue"));
      return;
    }
    setInvalid(null);
    setValue("");
    onAdd(q);
  };

  return (
    <div className="space-y-2 rounded-md border border-border p-3">
      <p className="text-caption font-medium text-fg-muted">{t("endpoints.condition.title")}</p>
      <div className="flex flex-wrap items-end gap-2">
        <Field id="condition-attr" label={t("endpoints.condition.attribute")} className="min-w-40">
          <Select id="condition-attr" value={attr} onChange={(event) => setAttr(event.target.value)}>
            {attributes.length === 0 ? <option value="">{t("endpoints.condition.noAttributes")}</option> : null}
            {attributes.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="condition-op" label={t("endpoints.condition.operator")} className="min-w-36">
          <Select
            id="condition-op"
            value={op}
            onChange={(event) => setOp(event.target.value as FilterOp)}
          >
            {CONDITION_OPS.map((candidate) => (
              <option key={candidate} value={candidate}>
                {t(`entityGrid.ops.${candidate === "empty" ? "isEmpty" : candidate}`)}
              </option>
            ))}
          </Select>
        </Field>
        {needsValue ? (
          <Field id="condition-value" label={t("endpoints.condition.value")} className="min-w-40">
            <Input
              id="condition-value"
              value={value}
              onChange={(event) => setValue(event.target.value)}
            />
          </Field>
        ) : null}
        <Button type="button" variant="secondary" onClick={add}>
          {t("endpoints.condition.add")}
        </Button>
      </div>
      {invalid ? (
        <p role="alert" className="text-caption text-danger">
          {invalid}
        </p>
      ) : null}
      <p className="text-caption text-fg-muted">{t("endpoints.condition.hint")}</p>
    </div>
  );
}

/** The endpoint spec as this page reads it; the manifest refuses anything not in `EndpointSpec`. */
interface EndpointSpec {
  contextSpaceRef?: unknown;
  slug?: string;
  audience?: string;
  allowedProjects?: string[];
  policyRef?: string;
  enabledRepresentations?: string[];
  rateLimits?: { requestsPerMinute?: number; burst?: number };
  fileLimits?: { maxFileRows?: number; maxFileBytes?: number };
  caching?: { maxAgeSeconds?: number };
  projection?: { hiddenAttributes?: string[] };
  projectionRef?: unknown;
  viewMappingRef?: unknown;
  publish?: {
    ckan?: { instanceRef?: unknown; organization?: string; name?: string };
  };
}

/** The four narrowings a `ModelProjection` can carry (MP-01); an endpoint has none of its own. */
export const FILTER_KEYS = ["q", "scopeQ", "geoQ", "temporalQ"] as const;

export function filterOf(projection: Manifest): Record<string, string | undefined> {
  const filter = (projection.spec as { filter?: Record<string, string> }).filter ?? {};
  return filter;
}

export function classesOf(projection: Manifest): string[] {
  const classes = (projection.spec as { classes?: Array<{ name?: string }> }).classes ?? [];
  return classes.map((klass) => klass.name ?? "").filter((klass) => klass !== "");
}

function Section({
  title,
  lead,
  children,
}: {
  title: string;
  lead?: string;
  children: ReactNode;
}): JSX.Element {
  return (
    <section className="space-y-2">
      <h2 className="text-base font-semibold">{title}</h2>
      {lead ? <p className="text-sm text-fg-muted">{lead}</p> : null}
      {children}
    </section>
  );
}

/** The facts of one section: two columns on a screen, stacked under 400 px. */
function Facts({ children }: { children: ReactNode }): JSX.Element {
  return <dl className="grid gap-3 sm:grid-cols-2">{children}</dl>;
}

function Fact({ label, children }: { label: string; children: ReactNode }): JSX.Element {
  return (
    <div className="rounded-md border border-border p-3">
      <dt className="text-caption font-medium text-fg-muted">{label}</dt>
      <dd className="mt-1 text-sm text-fg">{children}</dd>
    </div>
  );
}

function Missing(): JSX.Element {
  const { t } = useTranslation();
  return <span className="text-fg-muted">{t("endpoints.page.notSet")}</span>;
}
