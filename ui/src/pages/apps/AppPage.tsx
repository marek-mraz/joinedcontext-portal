import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { AgentRunPage } from "./AgentRunPage";
import { AppGenerator } from "./AppGenerator";
import type { AgentRun } from "./useAgentRun";

/**
 * An application and its runs (AP-68, AP-69, T-0559).
 *
 * Loads the application's runs, showing the newest with `AgentRunPage` which replays the
 * conversation and attaches the live stream. If no run exists yet, shows `AppGenerator`
 * prefilled with the application name.
 */
export function AppPage({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { data, isPending, isError, error } = useQuery({
    queryKey: ["projects", project, "agent-runs", name],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project }, query: { app: name } as never },
        }),
      ),
  });

  if (isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (isError) {
    return (
      <p role="alert" className="text-danger">
        {error instanceof ApiError ? (error.problem?.detail ?? error.message) : t("app.error.generic")}
      </p>
    );
  }

  const items = (data?.items ?? []) as unknown as AgentRun[];
  const newest = items[0];

  if (newest) {
    return (
      <AgentRunPage
        project={project}
        runId={newest.id}
        onClose={() => {
          void navigate({ to: "/projects/$project/$plural", params: { project, plural: "apps" } });
        }}
      />
    );
  }

  return (
    <div className="space-y-3">
      <button
        type="button"
        onClick={() => {
          void navigate({ to: "/projects/$project/$plural", params: { project, plural: "apps" } });
        }}
        className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
      >
        {t("apps.back")}
      </button>
      <AppGenerator project={project} initialName={name} />
    </div>
  );
}
