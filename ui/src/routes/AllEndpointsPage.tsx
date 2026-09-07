import type { JSX } from "react";
import { useQueries } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import { useProjects } from "../api/projects";
import {
  catalogueUrl,
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
} from "../components/endpoints/links";
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

const SPACE_LABEL = "joinedcontext.com/space";
const COLUMNS = 5;

const AUDIENCE_TONE: Record<string, "neutral" | "info" | "warning"> = {
  "project-list": "neutral",
  organization: "info",
  public: "warning",
};

/**
 * Every Endpoint of every project the repository holds, in one table (EP-08, EP-44): the
 * project and space it publishes, its audience, and one direct link per representation.
 * Each project's list is the same query the project's own Endpoints page runs.
 */
export function AllEndpointsPage(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const projects = useProjects();
  const names = projects.data ?? [];

  const lists = useQueries({
    queries: names.map((project) => ({
      queryKey: queryKeys.list(project, "endpoints"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "endpoints" } },
          }),
        ),
    })),
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

  if (projects.isPending || lists.some((list) => list.isPending)) {
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

  const failed = projects.isError ? projects : lists.find((list) => list.isError);
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

  const rows = names.flatMap((project, index) =>
    asManifests(lists[index]?.data?.items ?? []).map((endpoint) => ({ project, endpoint })),
  );

  return (
    <div className="flex flex-col gap-section">
      {header}
      <Table caption={t("allEndpoints.title")}>
        {head}
        <TableBody>
          {rows.length === 0 ? (
            <TableEmpty columns={COLUMNS}>
              <EmptyState bare icon="endpoints" title={t("allEndpoints.empty")} />
            </TableEmpty>
          ) : (
            rows.map(({ project, endpoint }) => {
              const spec = endpoint.spec as {
                contextSpaceRef?: string;
                slug?: string;
                audience?: string;
                enabledRepresentations?: string[];
              };
              const space = spec.contextSpaceRef ?? endpoint.metadata.labels?.[SPACE_LABEL];
              const slug = spec.slug ?? "";
              const audience = spec.audience ?? "project-list";
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
                    <Badge tone={AUDIENCE_TONE[audience] ?? "neutral"}>
                      {t(`endpoints.audience.${audience}`)}
                    </Badge>
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
