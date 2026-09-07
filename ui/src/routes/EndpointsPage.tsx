import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { useProjects } from "../api/projects";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ChangeNotice } from "../components/ChangeNotice";
import { ExportButton } from "../components/export/ExportButton";
import { SchemaProjectionPanel } from "../pages/endpoints/SchemaProjectionPanel";
import {
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
} from "../components/endpoints/links";
import {
  admits,
  referenceManifest,
  referenceTo,
  SharedWithBadge,
  SPACE_LABEL,
  spaceOf,
} from "../components/endpoints/sharing";
import { endpointSchema, endpointUiSchema, generateSlug } from "../schemas/kinds";
import type { JsonSchema } from "../components/forms/types";
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

interface EndpointForm {
  name: string;
  title?: Record<string, string>;
  contextSpaceRef: string;
  slug: string;
  audience: string;
  enabledRepresentations: string[];
  allowedProjects?: string[];
  rateLimits?: { requestsPerMinute?: number; burst?: number };
  caching?: { maxAgeSeconds?: number };
}

/**
 * The spec the manifest takes, with the optional blocks the steward left alone removed.
 *
 * rjsf renders a nested object whether or not anybody fills it in, so an untouched fieldset
 * arrives as `{}` or as a half-filled object; either one fails `EndpointSpec::validate` on
 * the way in. `allowedProjects` goes the other way: the manifest requires it for
 * `project-list` and refuses it for the other two audiences (EP-14, EP-15).
 */
function toSpec(form: EndpointForm, hiddenAttributes: string[]) {
  const { allowedProjects, rateLimits, caching, ...rest } = form;
  // `name` and `title` are metadata, not spec.
  delete (rest as Partial<EndpointForm>).name;
  delete (rest as Partial<EndpointForm>).title;
  return {
    ...rest,
    ...(form.audience === "project-list" && allowedProjects && allowedProjects.length > 0
      ? { allowedProjects }
      : {}),
    ...(typeof rateLimits?.requestsPerMinute === "number"
      ? {
          rateLimits: {
            requestsPerMinute: rateLimits.requestsPerMinute,
            ...(typeof rateLimits.burst === "number" ? { burst: rateLimits.burst } : {}),
          },
        }
      : {}),
    ...(typeof caching?.maxAgeSeconds === "number"
      ? { caching: { maxAgeSeconds: caching.maxAgeSeconds } }
      : {}),
    ...(hiddenAttributes.length > 0 ? { projection: { hiddenAttributes } } : {}),
  };
}

function toEnvelope(project: string, form: EndpointForm, hiddenAttributes: string[]) {
  const { name, title } = form;
  const spec = toSpec(form, hiddenAttributes);
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name,
      namespace: project,
      // The Endpoint path template carries a {space}; the label is where the API reads it.
      labels: { [SPACE_LABEL]: form.contextSpaceRef },
      ...(title && Object.keys(title).length > 0 ? { title } : {}),
    },
    spec,
  };
}

function toForm(endpoint: Manifest): EndpointForm {
  const spec = endpoint.spec as {
    contextSpaceRef?: string;
    slug?: string;
    audience?: string;
    enabledRepresentations?: string[];
    allowedProjects?: string[];
    rateLimits?: { requestsPerMinute?: number; burst?: number };
    caching?: { maxAgeSeconds?: number };
  };
  return {
    name: endpoint.metadata.name,
    title: endpoint.metadata.title ?? undefined,
    contextSpaceRef: spec.contextSpaceRef ?? "",
    slug: spec.slug ?? "",
    audience: spec.audience ?? "project-list",
    enabledRepresentations: spec.enabledRepresentations ?? [],
    allowedProjects: spec.allowedProjects ?? [],
    rateLimits: spec.rateLimits,
    caching: spec.caching,
  };
}

/** The attributes an Endpoint holds back, `spec.projection.hiddenAttributes` (EP-61). */
function hiddenOf(endpoint: Manifest): string[] {
  const projection = (endpoint.spec as { projection?: { hiddenAttributes?: string[] } })
    .projection;
  return projection?.hiddenAttributes ?? [];
}

function CopyUrlButton({ slug }: { slug: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const url = `${window.location.origin}/api/endpoint/${slug}`;

  return (
    <Button
      size="sm"
      title={url}
      icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
      onClick={() => {
        void navigator.clipboard
          ?.writeText(url)
          .then(() => setCopied(true))
          .catch(() => setCopied(false));
      }}
    >
      {copied ? t("endpoints.copied") : t("endpoints.copyUrl")}
    </Button>
  );
}

const COLUMNS = 5;
const SHARED_COLUMNS = 4;

/** `schema` without `allowedProjects`: the manifest refuses the list outside `project-list`. */
function withoutAllowedProjects(schema: JsonSchema): JsonSchema {
  const properties = Object.fromEntries(
    Object.entries(schema.properties ?? {}).filter(([key]) => key !== "allowedProjects"),
  );
  return { ...schema, properties };
}

/** Endpoints of one project: who may call them, in which representations, and their public URL. */
export function EndpointsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const [editing, setEditing] = useState<EndpointForm | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  // Kept beside the form rather than in it: the panel names the attributes the endpoint
  // already publishes, which the form's own schema knows nothing about (EP-61).
  const [hidden, setHidden] = useState<string[]>([]);

  const list = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const spacesQuery = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });

  // The other projects of the repository: what the allowed-project picker offers, and whose
  // endpoints the "shared with this project" section reads (PF-05, EP-14).
  const projects = useProjects();
  const others = (projects.data ?? []).filter((name) => name !== project);
  const otherLists = useQueries({
    queries: others.map((other) => ({
      queryKey: queryKeys.list(other, "endpoints"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project: other, plural: "endpoints" } },
          }),
        ),
    })),
  });
  const referencesQuery = useQuery({
    queryKey: queryKeys.list(project, "shared"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "shared" } },
        }),
      ),
  });

  const propose = useMutation({
    mutationFn: async ({ form, create }: { form: EndpointForm; create: boolean }) => {
      setFormError(null);
      const body = toEnvelope(project, form, hidden) as never;
      const result = create
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "endpoints" } },
            body,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural: "endpoints", name: form.name } },
            body,
          });
      return unwrap(result);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setEditing(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "endpoints") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  // One click declares the SharedSpaceReference; it lands as a Change like every write (EP-15).
  const reference = useMutation({
    mutationFn: async ({ source, endpoint }: { source: string; endpoint: Manifest }) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "shared" } },
          body: referenceManifest(project, source, endpoint) as never,
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "shared") });
    },
  });

  const head = (
    <TableHead>
      <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.audience")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.representations")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.phase")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
    </TableHead>
  );

  if (list.isPending) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("endpoints.title")} description={t("endpoints.lead")} />
        <Table caption={t("endpoints.title")} status={t("app.loading")}>
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
        <PageHeader title={t("endpoints.title")} description={t("endpoints.lead")} />
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

  const endpoints = asManifests(list.data.items ?? []);
  const spaceNames = asManifests(spacesQuery.data?.items ?? []).map((s) => s.metadata.name);
  const references = asManifests(referencesQuery.data?.items ?? []);
  // Only what the gateway would admit this project to (resolver.rs): organization-wide, public,
  // or a project-list that names it.
  const shared = others.flatMap((source, index) =>
    asManifests(otherLists[index]?.data?.items ?? [])
      .filter((endpoint) => admits(endpoint, source, project))
      .map((endpoint) => ({ source, endpoint })),
  );
  // Projects an existing endpoint already lists stay pickable even when the list API no
  // longer knows them; otherwise the enum would refuse the value the manifest carries. With
  // no other project known the field stays free text, so nothing typed turns into a checkbox.
  const pickable =
    others.length > 0
      ? [
          ...others,
          ...(editing?.allowedProjects ?? []).filter(
            (name) => typeof name === "string" && name.length > 0 && !others.includes(name),
          ),
        ]
      : [];
  const baseSchema = endpointSchema(t, spaceNames, pickable);
  const schema =
    editing?.audience === "project-list" ? baseSchema : withoutAllowedProjects(baseSchema);
  const uiSchema = {
    ...endpointUiSchema,
    ...(pickable.length > 0 ? { allowedProjects: { "ui:widget": "checkboxes" } } : {}),
  };

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("endpoints.title")}
        description={t("endpoints.lead")}
        actions={
          <Button
            variant="primary"
            icon={<Icon name="plus" className="size-4" />}
            onClick={() => {
              setFormError(null);
              setIsNew(true);
              setHidden([]);
              setEditing({
                name: "",
                contextSpaceRef: spaceNames[0] ?? "",
                slug: generateSlug(),
                audience: "project-list",
                enabledRepresentations: ["ngsi-ld"],
                allowedProjects: [],
              });
            }}
          >
            {t("endpoints.add")}
          </Button>
        }
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <Table caption={t("endpoints.title")}>
        {head}
        <TableBody>
          {endpoints.length === 0 ? (
            <TableEmpty columns={COLUMNS}>
              <EmptyState
                bare
                icon="endpoints"
                title={t("endpoints.empty")}
                description={t("endpoints.addHint")}
              />
            </TableEmpty>
          ) : (
            endpoints.map((endpoint) => {
              const spec = endpoint.spec as {
                slug?: string;
                enabledRepresentations?: string[];
              };
              const slug = spec.slug ?? "";
              return (
                <TableRow key={endpoint.metadata.name}>
                  <TableCell primary>
                    <div>{localized(endpoint.metadata.title, locale, endpoint.metadata.name)}</div>
                    {endpoint.metadata.title ? (
                      <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                        {endpoint.metadata.name}
                      </div>
                    ) : null}
                  </TableCell>
                  <TableCell>
                    <SharedWithBadge endpoint={endpoint} />
                  </TableCell>
                  <TableCell>
                    <ul className="flex flex-wrap gap-1">
                      {(spec.enabledRepresentations ?? []).map((rep) => (
                        <li key={rep}>
                          {slug && REPRESENTATION_PATHS[rep] ? (
                            <EndpointLink href={endpointUrl(slug, REPRESENTATION_PATHS[rep])}>
                              {rep}
                            </EndpointLink>
                          ) : (
                            <Badge mono>{rep}</Badge>
                          )}
                        </li>
                      ))}
                    </ul>
                    {slug ? (
                      <ul className="mt-1.5 flex flex-wrap gap-1">
                        {ENDPOINT_LINKS.map((link) => (
                          <li key={link.key}>
                            <a
                              href={endpointUrl(slug, link.path)}
                              target="_blank"
                              rel="noreferrer"
                              title={endpointUrl(slug, link.path)}
                              className="focus-ring inline-flex items-center gap-1 rounded-full border border-border px-2 py-0.5 font-mono text-caption text-fg-muted hover:border-border-strong hover:bg-surface-muted hover:text-fg"
                            >
                              {t(`endpoints.link.${link.key}`)}
                              <Icon name="external" className="size-3" />
                            </a>
                          </li>
                        ))}
                      </ul>
                    ) : null}
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge kind="phase" value={endpoint.status?.phase} />
                  </TableCell>
                  <TableCell align="right">
                    <div className="flex flex-wrap items-center justify-end gap-1.5">
                      {spec.slug ? <CopyUrlButton slug={spec.slug} /> : null}
                      <ExportButton
                        project={project}
                        target={{ plural: "endpoints", name: endpoint.metadata.name }}
                        label={t("export.action")}
                        size="sm"
                      />
                      <Button
                        size="sm"
                        onClick={() => {
                          setFormError(null);
                          setIsNew(false);
                          setHidden(hiddenOf(endpoint));
                          setEditing(toForm(endpoint));
                        }}
                      >
                        {t("endpoints.edit")}
                      </Button>
                      {endpoint.status?.sourceUrl ? (
                        <a
                          href={endpoint.status.sourceUrl}
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

      <section aria-labelledby="shared-with-project" className="flex flex-col gap-3">
        <div>
          <h2 id="shared-with-project" className="text-title font-semibold text-fg">
            {t("endpoints.shared.title")}
          </h2>
          <p className="mt-1 text-body text-fg-muted">{t("endpoints.shared.lead")}</p>
        </div>
        <Table
          caption={t("endpoints.shared.title")}
          status={
            projects.isPending || otherLists.some((other) => other.isPending)
              ? t("app.loading")
              : undefined
          }
        >
          <TableHead>
            <TableHeaderCell>{t("endpoints.shared.source")}</TableHeaderCell>
            <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
            <TableHeaderCell>{t("endpoints.field.representations")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("endpoints.shared.reference")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {shared.length === 0 ? (
              <TableEmpty columns={SHARED_COLUMNS}>
                <EmptyState bare icon="globe" title={t("endpoints.shared.empty")} />
              </TableEmpty>
            ) : (
              shared.map(({ source, endpoint }) => {
                const spec = endpoint.spec as { slug?: string; enabledRepresentations?: string[] };
                const slug = spec.slug ?? "";
                const space = spaceOf(endpoint);
                const declared = slug ? referenceTo(references, slug) : undefined;
                const label = localized(endpoint.metadata.title, locale, endpoint.metadata.name);
                return (
                  <TableRow key={`${source}/${endpoint.metadata.name}`}>
                    <TableCell>
                      <div className="flex flex-col gap-0.5">
                        <Link
                          to="/projects/$project/$plural"
                          params={{ project: source, plural: "endpoints" }}
                          className="focus-ring rounded-sm font-medium text-fg hover:underline"
                        >
                          {source}
                        </Link>
                        {space ? (
                          <Link
                            to="/projects/$project/spaces/$name"
                            params={{ project: source, name: space }}
                            className="focus-ring inline-flex items-center gap-1 rounded-sm font-mono text-caption text-primary hover:underline"
                          >
                            {space}
                            <Icon name="chevronRight" className="size-3.5" />
                          </Link>
                        ) : null}
                      </div>
                    </TableCell>
                    <TableCell primary>
                      <div>{label}</div>
                      {endpoint.metadata.title ? (
                        <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                          {endpoint.metadata.name}
                        </div>
                      ) : null}
                      <div className="mt-1">
                        <SharedWithBadge endpoint={endpoint} />
                      </div>
                    </TableCell>
                    <TableCell>
                      <ul className="flex flex-wrap gap-1">
                        {(spec.enabledRepresentations ?? []).map((rep) => (
                          <li key={rep}>
                            {slug && REPRESENTATION_PATHS[rep] ? (
                              <EndpointLink href={endpointUrl(slug, REPRESENTATION_PATHS[rep])}>
                                {rep}
                              </EndpointLink>
                            ) : (
                              <Badge mono>{rep}</Badge>
                            )}
                          </li>
                        ))}
                      </ul>
                      {slug ? (
                        <ul className="mt-1.5 flex flex-wrap gap-1">
                          {ENDPOINT_LINKS.map((link) => (
                            <li key={link.key}>
                              <EndpointLink muted href={endpointUrl(slug, link.path)}>
                                {t(`endpoints.link.${link.key}`)}
                              </EndpointLink>
                            </li>
                          ))}
                        </ul>
                      ) : null}
                    </TableCell>
                    <TableCell align="right">
                      {declared ? (
                        <div className="flex flex-col items-end gap-0.5">
                          <Badge tone="success">{t("endpoints.shared.referenced")}</Badge>
                          <span className="font-mono text-caption text-fg-muted">
                            {t("endpoints.shared.alias")}:{" "}
                            {String((declared.spec as { alias?: string }).alias ?? "")}
                          </span>
                        </div>
                      ) : (
                        <Button
                          size="sm"
                          variant="primary"
                          aria-label={`${t("endpoints.shared.use")}: ${source}/${endpoint.metadata.name}`}
                          disabled={!slug || reference.isPending}
                          icon={<Icon name="plus" className="size-4" />}
                          onClick={() => reference.mutate({ source, endpoint })}
                        >
                          {t("endpoints.shared.use")}
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })
            )}
          </TableBody>
        </Table>
        {reference.isError ? (
          <Alert role="alert" tone="danger">
            {reference.error instanceof ApiError
              ? (reference.error.problem?.detail ?? reference.error.message)
              : t("app.error.generic")}
          </Alert>
        ) : null}
      </section>

      <ResourceFormDialog<EndpointForm>
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setEditing(null);
          }
        }}
        title={isNew ? t("endpoints.add") : t("endpoints.edit")}
        description={t("endpoints.addHint")}
        schema={schema}
        uiSchema={uiSchema}
        formData={editing ?? undefined}
        submitLabel={t("endpoints.propose")}
        disabled={propose.isPending}
        error={formError}
        onSubmit={(form) => propose.mutate({ form, create: isNew })}
        onChange={(form) => {
          // Keeps typed fields when the slug generator replaces one value of the form.
          if (form) {
            setEditing(form);
          }
        }}
      >
        {isNew ? (
          <div>
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() =>
                setEditing((prev) => (prev ? { ...prev, slug: generateSlug() } : prev))
              }
            >
              {t("endpoints.generateSlug")}
            </Button>
          </div>
        ) : null}
        {editing?.audience === "public" ? (
          <Alert role="note" tone="warning">
            {t("endpoints.publicNotice")}
          </Alert>
        ) : null}
        {editing && !isNew ? (
          <SchemaProjectionPanel slug={editing.slug} hidden={hidden} onHiddenChange={setHidden} />
        ) : null}
      </ResourceFormDialog>
    </div>
  );
}
