import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../api/client";
import { SpacesPage } from "./SpacesPage";
import { EndpointsPage } from "./EndpointsPage";
import { DashboardsPage } from "./DashboardsPage";
import { PipelinesPage } from "./PipelinesPage";
import { AccessPage } from "../pages/access/AccessPage";
import { FlowGallery } from "../pages/flows/Gallery";

/** `/api/v1/projects/{project}/{plural}` rendered as a plain table; MF-11…MF-15. */
export function ResourceListPage({
  project,
  plural,
}: {
  project: string;
  plural: string;
}): React.JSX.Element {
  // Some kinds have a view of their own; the rest fall back to the plain manifest table.
  if (plural === "spaces") {
    return <SpacesPage project={project} />;
  }
  if (plural === "endpoints") {
    return <EndpointsPage project={project} />;
  }
  if (plural === "pipelines") {
    return <PipelinesPage project={project} />;
  }
  if (plural === "dashboards") {
    return <DashboardsPage project={project} />;
  }
  // "flows" is a section too: the gallery reads organization-level Blueprints, not a project
  // collection, and the wizard writes through /flows rather than a resource route (CC-30).
  if (plural === "flows") {
    return <FlowGallery project={project} />;
  }
  // "access" is a section, not a kind: ServiceAccounts and the caller's own grants (PF-40, EP-60).
  if (plural === "access") {
    return <AccessPage project={project} />;
  }
  return <GenericListPage project={project} plural={plural} />;
}

function GenericListPage({
  project,
  plural,
}: {
  project: string;
  plural: string;
}): React.JSX.Element {
  const { t } = useTranslation();
  const list = useQuery({
    queryKey: queryKeys.list(project, plural),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });

  if (list.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (list.isError) {
    return (
      <div role="alert">
        <p>{t("app.error.generic")}</p>
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

  const items = list.data.items ?? [];
  return (
    <table className="w-full border-collapse text-sm">
      <caption className="sr-only">{plural}</caption>
      <thead>
        <tr className="border-b border-border text-left">
          <th scope="col" className="py-2 pr-4">
            name
          </th>
          <th scope="col" className="py-2 pr-4">
            kind
          </th>
        </tr>
      </thead>
      <tbody>
        {items.map((item) => (
          <tr key={`${item.kind}/${item.metadata.name}`} className="border-b border-border">
            <td className="py-2 pr-4 font-mono">{item.metadata.name}</td>
            <td className="py-2 pr-4">{item.kind}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
