import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError, api, queryKeys, unwrap } from "../../api/client";
import { useAuth } from "../../auth/AuthProvider";
import { Button } from "../ui";
import { useWorkspace } from "./WorkspaceContext";

export function WorkspaceBar({ project }: { project: string }): React.JSX.Element | null {
  const { t } = useTranslation();
  const { name, leave } = useWorkspace();
  const { identity } = useAuth();

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

  // Gone (404), expired, or not the caller's to see (403): one line and the way out (UI-61).
  const notice = (message: string) => (
    <div
      role="region"
      aria-label={t("workspaces.bar.label")}
      className="flex items-center gap-3 border-b border-warning/30 bg-warning-soft px-4 py-2 text-sm"
    >
      <span>{message}</span>
      <Button size="sm" variant="ghost" onClick={leave}>
        {t("workspaces.bar.leave")}
      </Button>
    </div>
  );
  if (workspace.error instanceof ApiError && workspace.error.status === 403) {
    return notice(t("workspaces.bar.refused", { reason: workspace.error.message }));
  }
  if (workspace.isError) return notice(t("workspaces.bar.gone"));

  if (!workspace.data) return null;

  const { title, name: wsName, createdAt, expiresAt, owner, changes } = workspace.data;
  // Expiry is judged at the moment the copy was read, so the render stays pure.
  if (new Date(expiresAt).getTime() <= workspace.dataUpdatedAt) {
    return notice(t("workspaces.bar.expired", { date: new Date(expiresAt).toLocaleDateString() }));
  }
  const mine = !!identity && (owner === identity.email || owner === identity.username);
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
      <span className="text-fg-muted" data-testid="workspace-changes">
        {t("workspaces.bar.changes", { count: changeCount })}
      </span>
      {mine ? null : (
        <span className="rounded-md border border-border px-2 py-0.5 text-xs" data-testid="workspace-foreign">
          {t("workspaces.bar.foreign", { owner })}
        </span>
      )}
      <div className="ml-auto flex items-center gap-2">
        <Link
          to="/projects/$project/workspaces/$name/compare"
          params={{ project, name: wsName }}
          className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
        >
          {t("workspaces.bar.compare")}
        </Link>
        {mine && changeCount > 0 ? (
          <Link
            to="/projects/$project/workspaces/$name/bring-back"
            params={{ project, name: wsName }}
            className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
          >
            {t("workspaces.bar.bringBack")}
          </Link>
        ) : null}
        <Button size="sm" variant="ghost" onClick={leave}>
          {t("workspaces.bar.leave")}
        </Button>
      </div>
    </div>
  );
}
