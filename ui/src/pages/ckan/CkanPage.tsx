import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { isChange } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import type { components } from "../../api/schema";

type CkanStatus = components["schemas"]["CkanStatus"];
type PublicationStatus = components["schemas"]["PublicationStatus"];

export function ckanStatusKey(project: string) {
  return ["projects", project, "ckan", "status"] as const;
}

/**
 * The CKAN view of one project: which catalogues it can publish to, and what each endpoint
 * becomes in them (T-0319, EP-62…EP-67).
 *
 * The API token is never a field of this form. A `CkanInstance` names a `secretRef` and the
 * Portal refuses a manifest carrying a literal credential (MF-24, CC-06), so what a steward
 * enters here is the name of the secret the operator loaded into the secret store; the value
 * itself never passes through a browser and never reaches Git (EP-67).
 */
export function CkanPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [change, setChange] = useState<Change | null>(null);

  const status = useQuery({
    queryKey: ckanStatusKey(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/ckan/status", {
          params: { path: { project } },
        }),
      ),
  });

  const create = useMutation({
    mutationFn: async (instance: InstanceDraft) => {
      const body = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "CkanInstance",
        metadata: { name: instance.name, namespace: project },
        spec: {
          url: instance.url,
          ...(instance.organizationDefault
            ? { organizationDefault: instance.organizationDefault }
            : {}),
          apiTokenRef: { name: instance.secretName, key: instance.secretKey || "apiToken" },
        },
      };
      return unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "ckaninstances" } },
          body: body as never,
        }),
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: ckanStatusKey(project) });
    },
  });

  return (
    <section aria-labelledby="ckan-heading" className="space-y-8">
      <h2 id="ckan-heading" className="text-xl font-bold">
        {t("ckan.title")}
      </h2>
      <p className="max-w-2xl text-sm">{t("ckan.intro")}</p>

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {create.error ? (
        <p role="alert" className="text-sm text-danger">
          {create.error instanceof ApiError ? create.error.message : t("app.error.generic")}
        </p>
      ) : null}

      <Instances
        instances={status.data?.instances ?? []}
        loading={status.isLoading}
        onSubmit={(draft) => create.mutate(draft)}
        submitting={create.isPending}
      />
      <Publications
        publications={status.data?.publications ?? []}
        loading={status.isLoading}
      />
    </section>
  );
}

interface InstanceDraft {
  name: string;
  url: string;
  organizationDefault: string;
  secretName: string;
  secretKey: string;
}

const EMPTY_DRAFT: InstanceDraft = {
  name: "",
  url: "https://",
  organizationDefault: "",
  secretName: "",
  secretKey: "apiToken",
};

function Instances({
  instances,
  loading,
  onSubmit,
  submitting,
}: {
  instances: CkanStatus["instances"];
  loading: boolean;
  onSubmit: (draft: InstanceDraft) => void;
  submitting: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<InstanceDraft>(EMPTY_DRAFT);

  return (
    <section aria-labelledby="ckan-instances" className="space-y-3">
      <h3 id="ckan-instances" className="text-lg font-semibold">
        {t("ckan.instances.title")}
      </h3>
      {loading ? <p role="status">{t("app.loading")}</p> : null}
      {!loading && instances.length === 0 ? <p>{t("ckan.instances.empty")}</p> : null}
      {instances.length > 0 ? (
        <table className="w-full text-left text-sm">
          <thead>
            <tr className="border-b border-border">
              <th scope="col" className="py-1">{t("ckan.instances.name")}</th>
              <th scope="col" className="py-1">{t("ckan.instances.url")}</th>
              <th scope="col" className="py-1">{t("ckan.instances.organization")}</th>
              <th scope="col" className="py-1">{t("ckan.instances.tokenRef")}</th>
            </tr>
          </thead>
          <tbody>
            {instances.map((instance) => (
              <tr key={instance.name} className="border-b border-border">
                <td className="py-1 font-mono">{instance.name}</td>
                <td className="py-1">
                  <a
                    href={instance.url}
                    className="underline hover:no-underline"
                    rel="noreferrer"
                    target="_blank"
                  >
                    {instance.url}
                  </a>
                </td>
                <td className="py-1">{instance.organizationDefault ?? "—"}</td>
                <td className="py-1 font-mono">{instance.apiTokenRef}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}

      <form
        className="max-w-xl space-y-2"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(draft);
          setDraft(EMPTY_DRAFT);
        }}
      >
        <Field
          id="ckan-instance-name"
          label={t("ckan.instances.name")}
          value={draft.name}
          onChange={(name) => setDraft({ ...draft, name })}
          required
        />
        <Field
          id="ckan-instance-url"
          label={t("ckan.instances.url")}
          value={draft.url}
          onChange={(url) => setDraft({ ...draft, url })}
          type="url"
          required
        />
        <Field
          id="ckan-instance-org"
          label={t("ckan.instances.organization")}
          value={draft.organizationDefault}
          onChange={(organizationDefault) => setDraft({ ...draft, organizationDefault })}
        />
        <Field
          id="ckan-instance-secret"
          label={t("ckan.instances.tokenRef")}
          value={draft.secretName}
          onChange={(secretName) => setDraft({ ...draft, secretName })}
          required
        />
        <p className="text-sm">{t("ckan.instances.tokenHelp")}</p>
        <button
          type="submit"
          disabled={submitting}
          className="rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-60"
        >
          {t("ckan.instances.propose")}
        </button>
      </form>
    </section>
  );
}

function Field({
  id,
  label,
  value,
  onChange,
  type = "text",
  required = false,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  type?: string;
  required?: boolean;
}): JSX.Element {
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-sm font-medium">
        {label}
      </label>
      <input
        id={id}
        type={type}
        value={value}
        required={required}
        onChange={(event) => onChange(event.target.value)}
        className="rounded border border-border bg-surface px-2 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus"
      />
    </div>
  );
}

function Publications({
  publications,
  loading,
}: {
  publications: PublicationStatus[];
  loading: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="ckan-publications" className="space-y-3">
      <h3 id="ckan-publications" className="text-lg font-semibold">
        {t("ckan.publications.title")}
      </h3>
      {loading ? <p role="status">{t("app.loading")}</p> : null}
      {!loading && publications.length === 0 ? <p>{t("ckan.publications.empty")}</p> : null}
      <ul className="space-y-4">
        {publications.map((publication) => (
          <li key={publication.endpoint} className="rounded border border-border p-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono font-semibold">{publication.endpoint}</span>
              <StatusChip publication={publication} />
              {publication.datasetUrl ? (
                <a
                  href={publication.datasetUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="underline hover:no-underline"
                >
                  {publication.dataset}
                </a>
              ) : (
                <span className="font-mono">{publication.dataset}</span>
              )}
            </div>
            {publication.datastore ? (
              <p className="mt-1 text-sm">
                {t("ckan.publications.datastore", {
                  representation: publication.datastore.representation,
                  refresh: publication.datastore.refresh,
                })}
              </p>
            ) : null}
            <ul className="mt-2 flex flex-wrap gap-2">
              {publication.resources.map((resource) => (
                <li key={resource.url}>
                  <a
                    href={resource.url}
                    target="_blank"
                    rel="noreferrer"
                    className="rounded border border-border px-2 py-1 text-sm underline hover:no-underline"
                  >
                    {resource.format}
                  </a>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Published, or the reason it is not: a dangling instance is the usual one. */
function StatusChip({ publication }: { publication: PublicationStatus }): JSX.Element {
  const { t } = useTranslation();
  const missing = publication.instanceMissing;
  return (
    <span
      className={
        missing
          ? "rounded border border-danger px-2 py-0.5 text-xs font-medium text-danger"
          : "rounded border border-border px-2 py-0.5 text-xs font-medium"
      }
    >
      {missing
        ? t("ckan.publications.missingInstance", { instance: publication.instance })
        : t("ckan.publications.publishedTo", { instance: publication.instance })}
    </span>
  );
}
