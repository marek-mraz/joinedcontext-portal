import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { isChange } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  Card,
  Field,
  Input,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";

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
      return proposeChecked(project, "ckaninstances", body as { metadata: { name: string } }, true);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: ckanStatusKey(project) });
    },
  });

  return (
    <section aria-label={t("ckan.title")} className="space-y-8">
      <PageHeader title={t("ckan.title")} description={t("ckan.intro")} />

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {create.error ? (
        <Alert role="alert" tone="danger">
          {create.error instanceof ApiError ? create.error.message : t("app.error.generic")}
        </Alert>
      ) : null}

      <Instances
        project={project}
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
  project,
  instances,
  loading,
  onSubmit,
  submitting,
}: {
  project: string;
  instances: CkanStatus["instances"];
  loading: boolean;
  onSubmit: (draft: InstanceDraft) => void;
  submitting: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<InstanceDraft>(EMPTY_DRAFT);

  return (
    <section aria-labelledby="ckan-instances" className="space-y-3">
      <h2 id="ckan-instances" className="text-lg font-semibold">
        {t("ckan.instances.title")}
      </h2>
      {loading ? <p role="status">{t("app.loading")}</p> : null}
      {!loading && instances.length === 0 ? <p>{t("ckan.instances.empty")}</p> : null}
      {instances.length > 0 ? (
        <Table caption={t("ckan.instances.title")}>
          <TableHead>
            <TableHeaderCell>{t("ckan.instances.name")}</TableHeaderCell>
            <TableHeaderCell>{t("ckan.instances.url")}</TableHeaderCell>
            <TableHeaderCell>{t("ckan.instances.organization")}</TableHeaderCell>
            <TableHeaderCell>{t("ckan.instances.tokenRef")}</TableHeaderCell>
            <TableHeaderCell align="right">
              <span className="sr-only">{t("approvals.actions")}</span>
            </TableHeaderCell>
          </TableHead>
          <TableBody>
            {instances.map((instance) => (
              <TableRow key={instance.name}>
                <TableCell className="font-mono">{instance.name}</TableCell>
                <TableCell>
                  <a
                    href={instance.url}
                    className="underline hover:no-underline"
                    rel="noreferrer"
                    target="_blank"
                  >
                    {instance.url}
                  </a>
                </TableCell>
                <TableCell>{instance.organizationDefault ?? "—"}</TableCell>
                <TableCell className="font-mono">{instance.apiTokenRef}</TableCell>
                <TableCell align="right">
                  <span className="inline-flex items-center gap-1.5">
                    <EditResourceAction
                      target={{ project, kind: "CkanInstance", plural: "ckaninstances", name: instance.name }}
                    />
                    <DeleteResourceAction
                      target={{ project, kind: "CkanInstance", plural: "ckaninstances", name: instance.name }}
                    />
                  </span>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}

      <form
        className="max-w-4xl space-y-2"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(draft);
          setDraft(EMPTY_DRAFT);
        }}
      >
        <Field id="ckan-instance-name" label={t("ckan.instances.name")}>
          <Input
            id="ckan-instance-name"
            value={draft.name}
            onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            required
          />
        </Field>
        <Field id="ckan-instance-url" label={t("ckan.instances.url")}>
          <Input
            id="ckan-instance-url"
            type="url"
            value={draft.url}
            onChange={(event) => setDraft({ ...draft, url: event.target.value })}
            required
          />
        </Field>
        <Field id="ckan-instance-org" label={t("ckan.instances.organization")}>
          <Input
            id="ckan-instance-org"
            value={draft.organizationDefault}
            onChange={(event) => setDraft({ ...draft, organizationDefault: event.target.value })}
          />
        </Field>
        <Field id="ckan-instance-secret" label={t("ckan.instances.tokenRef")}>
          <Input
            id="ckan-instance-secret"
            value={draft.secretName}
            onChange={(event) => setDraft({ ...draft, secretName: event.target.value })}
            required
          />
        </Field>
        <p className="text-sm">{t("ckan.instances.tokenHelp")}</p>
        {/* The catalogue is proposed as this project's `CkanInstance`, so that is the permission
            the control needs. Without the guard a viewer filled the form and met the 403 only
            after pressing it (T-2243, UI-44). */}
        <PermissionGuard project={project} kind="CkanInstance" verb="propose">
          <Button type="submit" variant="primary" disabled={submitting}>
            {t("ckan.instances.propose")}
          </Button>
        </PermissionGuard>
      </form>
    </section>
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
      <h2 id="ckan-publications" className="text-lg font-semibold">
        {t("ckan.publications.title")}
      </h2>
      {loading ? <p role="status">{t("app.loading")}</p> : null}
      {!loading && publications.length === 0 ? <p>{t("ckan.publications.empty")}</p> : null}
      <ul className="space-y-4">
        {publications.map((publication) => (
          <li key={publication.endpoint}>
            <Card className="p-3">
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
            </Card>
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
    <Badge tone={missing ? "danger" : "neutral"}>
      {missing
        ? t("ckan.publications.missingInstance", { instance: publication.instance })
        : t("ckan.publications.publishedTo", { instance: publication.instance })}
    </Badge>
  );
}
