import type { JSX } from "react";
import { ResourceList } from "../components/ResourceList";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, whilePending } from "../api/client";
import { asManifests, localized } from "../api/manifest";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import {
  EmptyState,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../components/ui";
import { SpacesPage } from "./SpacesPage";
import { EndpointsPage } from "./EndpointsPage";
import { DashboardsPage } from "./DashboardsPage";
import { PipelinesPage } from "./PipelinesPage";
import { DataSourcesPage } from "../pages/datasources/DataSourcesPage";
import { AccessPage } from "../pages/access/AccessPage";
import { FlowGallery } from "../pages/flows/Gallery";
import { AppsCatalog } from "../pages/apps/AppsCatalog";
import { SyncSourcesPage } from "../pages/sync/SyncSourcesPage";

const VIEWS: Record<string, (props: { project: string; edit?: string }) => JSX.Element> = {
  spaces: SpacesPage,
  endpoints: EndpointsPage,
  pipelines: PipelinesPage,
  datasources: DataSourcesPage,
  dashboards: DashboardsPage,
  // "flows" is a section too: the gallery reads organization-level Blueprints, not a project
  // collection, and the wizard writes through /flows rather than a resource route (CC-30).
  flows: FlowGallery,
  // Apps are a kind, but a card catalogue with a preview frame and a publication action, not
  // a manifest table (AP-18, AP-19, AP-20).
  apps: AppsCatalog,
  // A SyncSource is a running loop as well as a manifest, so its view carries the phase, the
  // revision it carries and the three buttons of MF-30.
  syncsources: SyncSourcesPage,
  // "access" is a section, not a kind: ServiceAccounts and the caller's own grants (PF-40, EP-60).
  access: AccessPage,
};

/** `/api/v1/projects/{project}/{plural}`: a kind's own page, or its resources in a table (MF-11…MF-15). */
export function ResourceListPage({
  project,
  plural,
  edit,
}: {
  project: string;
  plural: string;
  /** `?edit=<name>`: open this kind's own editor on that resource at once (T-2281). */
  edit?: string;
}): JSX.Element {
  const View = VIEWS[plural];
  if (View) {
    return <View project={project} edit={edit} />;
  }
  return <GenericListPage project={project} plural={plural} />;
}

function GenericListPage({
  project,
  plural,
}: {
  project: string;
  plural: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const list = useQuery({
    queryKey: queryKeys.list(project, plural),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });

  const items = asManifests(list.data?.items ?? []);
  return (
    <ResourceList
      query={list}
      caption={plural}
      head={
        <TableHead>
          <TableHeaderCell>{t("resourceList.name")}</TableHeaderCell>
          <TableHeaderCell>{t("resourceList.phase")}</TableHeaderCell>
          <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
        </TableHead>
      }
      columns={3}
      count={items.length}
      empty={<EmptyState bare
            title={t("resourceList.empty")}
            description={t("resourceList.emptyHint")} />}
    >
      {items.map((item) => {
          const title = localized(item.metadata.title, locale, item.metadata.name);
          const target = { project, kind: item.kind, plural, name: item.metadata.name, label: title };
          return (
            <TableRow key={item.metadata.name}>
              <TableCell primary>
                <div>{title}</div>
                {item.metadata.title ? (
                  <div className="mt-0.5 font-mono text-caption text-fg-subtle">{item.metadata.name}</div>
                ) : null}
              </TableCell>
              <TableCell>
                <LifecycleBadge kind="phase" value={item.status?.phase} />
              </TableCell>
              <TableCell align="right">
                {/* One menu at the end of the row, for every kind that falls through here (T-2287). */}
                <ResourceRowActions project={project} target={target} />
              </TableCell>
            </TableRow>
          );
        })}
    </ResourceList>
  );
}
