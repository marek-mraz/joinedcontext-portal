import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { Button } from "../ui";
import { useWorkspace } from "./WorkspaceContext";

export function WorkspaceBar({ project }: { project: string }): React.JSX.Element | null {
  const { t } = useTranslation();
  const { name, leave } = useWorkspace();

  const workspace = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name],
    enabled: !!name,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}", {
          params: { path: { project, name: name! } },
        }),
      ),
    retry: false,
  });

  if (!name) return null;

  // 404 = expired or gone
  if (workspace.isError && (workspace.error as { status?: number })?.status === 404) {
    return (
      <div
        role="region"
        aria-label={t("workspaces.bar.label")}
        className="flex items-center gap-3 border-b border-warning/30 bg-warning-soft px-4 py-2 text-sm"
      >
        <span>{t("workspaces.bar.gone")}</span>
        <Button size="sm" variant="ghost" onClick={leave}>
          {t("workspaces.bar.leave")}
        </Button>
      </div>
    );
  }

  if (!workspace.data) return null;

  const { title, name: wsName, createdAt, changes } = workspace.data;
  const display = title ?? wsName;
  const date = new Date(createdAt).toLocaleDateString();
  const changeCount = changes ?? 0;

  return (
    <div
      role="region"
      aria-label={t("workspaces.bar.label")}
      className="flex flex-wrap items-center gap-3 border-b border-primary/30 bg-primary-soft px-4 py-2 text-sm"
    >
      <span className="font-medium">
        {t("workspaces.bar.message", { title: display, date })}
      </span>
      {changeCount > 0 ? (
        <span className="text-fg-muted">
          {t("workspaces.bar.changes", { count: changeCount })}
        </span>
      ) : null}
      <div className="ml-auto flex items-center gap-2">
        <Link
          to="/projects/$project/workspaces/$name/compare"
          params={{ project, name: wsName }}
          className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
        >
          {t("workspaces.bar.compare")}
        </Link>
        <Link
          to="/projects/$project/workspaces/$name/bring-back"
          params={{ project, name: wsName }}
          className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
        >
          {t("workspaces.bar.bringBack")}
        </Link>
        <Button size="sm" variant="ghost" onClick={leave}>
          {t("workspaces.bar.leave")}
        </Button>
      </div>
    </div>
  );
}
