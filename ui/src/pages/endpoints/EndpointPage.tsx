import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, api, queryKeys, unwrap, whilePending } from "../../api/client";
import { asManifests, localized, refName } from "../../api/manifest";
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
import { Badge, Button, PageHeader, SourceLink } from "../../components/ui";

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
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => {
                    void navigator.clipboard?.writeText(endpointUrl(slug, "")).catch(() => undefined);
                  }}
                >
                  {t("endpoints.copyUrl")}
                </Button>
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
            <Facts>
              {FILTER_KEYS.map((key) => (
                <Fact key={key} label={t(`endpoints.filter.${key}`)}>
                  {filterOf(projection)[key] ? (
                    <code className="break-all font-mono text-caption">{filterOf(projection)[key]}</code>
                  ) : (
                    <p className="text-caption text-fg-muted">{t("endpoints.page.filterEmpty")}</p>
                  )}
                  <p className="mt-1 text-caption text-fg-muted">{t(`endpoints.filter.${key}Help`)}</p>
                </Fact>
              ))}
            </Facts>
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
