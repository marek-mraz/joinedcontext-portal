import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import {
  catalogueUrl,
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
} from "../components/endpoints/links";
import { SharedWithBadge, spaceOf } from "../components/endpoints/sharing";
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
} from "../components/ui";

const COLUMNS = 5;

/**
 * Every Endpoint of every project the caller may read, in one table (EP-08, EP-44, PF-60): the
 * project and space it publishes, its audience, and one direct link per representation. The
 * organization-level route answers it in one request and decides what is in it — an org-admin
 * sees every project, a steward their own, and nobody sees a project they may not read (R20).
 */
export function AllEndpointsPage(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const endpoints = useQuery({
    queryKey: queryKeys.allEndpoints(),
    queryFn: async () => unwrap(await api.GET("/api/v1/endpoints")),
  });

  const head = (
    <TableHead>
      <TableHeaderCell>{t("allEndpoints.field.project")}</TableHeaderCell>
      <TableHeaderCell>{t("allEndpoints.field.space")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.audience")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.representations")}</TableHeaderCell>
    </TableHead>
  );
  const header = <PageHeader title={t("allEndpoints.title")} description={t("allEndpoints.lead")} />;

  if (endpoints.isPending) {
    return (
      <div className="flex flex-col gap-section">
        {header}
        <Table caption={t("allEndpoints.title")} status={t("app.loading")}>
          {head}
          <TableSkeleton columns={COLUMNS} />
        </Table>
      </div>
    );
  }

  const failed = endpoints.isError ? endpoints : null;
  if (failed) {
    const message =
      failed.error instanceof ApiError
        ? (failed.error.problem?.detail ?? failed.error.message)
        : t("app.error.generic");
    return (
      <div className="flex flex-col gap-section">
        {header}
        <Alert
          role="alert"
          tone="danger"
          actions={
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() => {
                void failed.refetch();
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

  const rows = asManifests(endpoints.data?.items ?? []).map((endpoint) => ({
    // The project is the manifest's own namespace, which is what the route answers with.
    project: endpoint.metadata.namespace ?? "",
    endpoint,
  }));

  return (
    <div className="flex flex-col gap-section">
      {header}
      <Table caption={t("allEndpoints.title")}>
        {head}
        <TableBody>
          {rows.length === 0 ? (
            <TableEmpty columns={COLUMNS}>
              <EmptyState bare
            icon="endpoints"
            title={t("allEndpoints.empty")}
            description={t("allEndpoints.emptyHint")} />
            </TableEmpty>
          ) : (
            rows.map(({ project, endpoint }) => {
              const spec = endpoint.spec as {
                slug?: string;
                enabledRepresentations?: string[];
              };
              const space = spaceOf(endpoint);
              const slug = spec.slug ?? "";
              const key = `${project}/${endpoint.metadata.name}`;
              return (
                <TableRow key={key}>
                  <TableCell>
                    <Link
                      to="/projects/$project/$plural"
                      params={{ project, plural: "endpoints" }}
                      className="focus-ring rounded-sm font-medium text-fg hover:underline"
                    >
                      {project}
                    </Link>
                  </TableCell>
                  <TableCell>
                    {space ? (
                      <div className="flex flex-col gap-0.5">
                        <span className="font-mono text-body text-fg">{space}</span>
                        <Link
                          to="/projects/$project/spaces/$name"
                          params={{ project, name: space }}
                          aria-label={`${t("spaces.inside.open")}: ${project}/${space}`}
                          className="focus-ring inline-flex items-center gap-1 rounded-sm text-caption text-primary hover:underline"
                        >
                          {t("spaces.inside.open")}
                          <Icon name="chevronRight" className="size-3.5" />
                        </Link>
                      </div>
                    ) : (
                      <span className="text-fg-subtle">—</span>
                    )}
                  </TableCell>
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
                            <EndpointLink muted href={endpointUrl(slug, link.path)}>
                              {t(`endpoints.link.${link.key}`)}
                            </EndpointLink>
                          </li>
                        ))}
                        <li>
                          <EndpointLink muted href={catalogueUrl(endpoint.metadata.name)}>
                            {t("endpoints.link.catalogue")}
                          </EndpointLink>
                        </li>
                      </ul>
                    ) : null}
                  </TableCell>
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>
    </div>
  );
}
