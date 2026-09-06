import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, isChange, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ChangeNotice } from "../components/ChangeNotice";
import { ExportButton } from "../components/export/ExportButton";
import { SchemaProjectionPanel } from "../pages/endpoints/SchemaProjectionPanel";
import { endpointSchema, endpointUiSchema, generateSlug } from "../schemas/kinds";

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

const SPACE_LABEL = "joinedcontext.com/space";

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
    <button
      type="button"
      onClick={() => {
        void navigator.clipboard
          ?.writeText(url)
          .then(() => setCopied(true))
          .catch(() => setCopied(false));
      }}
      title={url}
      className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
    >
      {copied ? t("endpoints.copied") : t("endpoints.copyUrl")}
    </button>
  );
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

  const endpoints = asManifests(list.data.items ?? []);
  const spaceNames = asManifests(spacesQuery.data?.items ?? []).map((s) => s.metadata.name);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <h1 className="text-xl font-bold">{t("endpoints.title")}</h1>
        <button
          type="button"
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
          className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
        >
          {t("endpoints.add")}
        </button>
      </div>

      {change ? <ChangeNotice change={change} project={project} /> : null}

      {endpoints.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("endpoints.empty")}</p>
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
                  {t("endpoints.field.phase")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium text-right">
                  {t("approvals.actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {endpoints.map((endpoint) => {
                const spec = endpoint.spec as {
                  slug?: string;
                  audience?: string;
                  enabledRepresentations?: string[];
                };
                return (
                  <tr key={endpoint.metadata.name} className="hover:bg-surface-subtle/50">
                    <td className="px-4 py-3">
                      <div className="font-medium">
                        {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
                      </div>
                      {endpoint.metadata.title ? (
                        <div className="font-mono text-xs text-surface-fg/60">
                          {endpoint.metadata.name}
                        </div>
                      ) : null}
                    </td>
                    <td className="px-4 py-3">
                      <span className="inline-flex items-center rounded border border-border bg-surface-subtle px-2 py-0.5 text-xs font-medium">
                        {t(`endpoints.audience.${spec.audience ?? "project-list"}`)}
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <ul className="flex flex-wrap gap-1">
                        {(spec.enabledRepresentations ?? []).map((rep) => (
                          <li
                            key={rep}
                            className="inline-flex items-center rounded border border-border px-2 py-0.5 font-mono text-xs"
                          >
                            {rep}
                          </li>
                        ))}
                      </ul>
                    </td>
                    <td className="px-4 py-3">
                      <LifecycleBadge kind="phase" value={endpoint.status?.phase} />
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex flex-wrap items-center justify-end gap-2">
                        {spec.slug ? <CopyUrlButton slug={spec.slug} /> : null}
                        <ExportButton
                          project={project}
                          target={{ plural: "endpoints", name: endpoint.metadata.name }}
                          label={t("export.action")}
                          className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                        />
                        <button
                          type="button"
                          onClick={() => {
                            setFormError(null);
                            setIsNew(false);
                            setHidden(hiddenOf(endpoint));
                            setEditing(toForm(endpoint));
                          }}
                          className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
                        >
                          {t("endpoints.edit")}
                        </button>
                        {endpoint.status?.sourceUrl ? (
                          <a
                            href={endpoint.status.sourceUrl}
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

      <ResourceFormDialog<EndpointForm>
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setEditing(null);
          }
        }}
        title={isNew ? t("endpoints.add") : t("endpoints.edit")}
        description={t("endpoints.addHint")}
        schema={endpointSchema(t, spaceNames)}
        uiSchema={endpointUiSchema}
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
          <button
            type="button"
            onClick={() =>
              setEditing((prev) => (prev ? { ...prev, slug: generateSlug() } : prev))
            }
            className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("endpoints.generateSlug")}
          </button>
        ) : null}
        {editing?.audience === "public" ? (
          <p role="note" className="rounded border border-border bg-surface-subtle p-3 text-sm">
            {t("endpoints.publicNotice")}
          </p>
        ) : null}
        {editing && !isNew ? (
          <SchemaProjectionPanel slug={editing.slug} hidden={hidden} onHiddenChange={setHidden} />
        ) : null}
      </ResourceFormDialog>
    </div>
  );
}
