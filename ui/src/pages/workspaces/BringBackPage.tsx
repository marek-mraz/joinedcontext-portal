import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { FileList } from "./ComparePage";
import { Alert, Button, EmptyState, PageHeader } from "../../components/ui";
import { useAuth } from "../../auth/AuthProvider";


function valueText(value: unknown): string {
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

export function BringBackPage({
  project,
  name,
}: {
  project: string;
  name: string;
}): JSX.Element {
  const { t } = useTranslation();
  const { identity } = useAuth();
  const queryClient = useQueryClient();
  const [resolutions, setResolutions] = useState<
    Record<string, "ours" | "theirs">
  >({});
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [proposeError, setProposeError] = useState<string | null>(null);
  const [proposedChange, setProposedChange] = useState<string | null>(null);

  const workspace = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}", {
          params: { path: { project, name } },
        }),
      ),
    retry: false,
  });

  const comparison = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name, "compare"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}/compare", {
          params: { path: { project, name } },
        }),
      ),
  });

  const isOwner =
    identity &&
    (identity.email === workspace.data?.owner ||
      identity.username === workspace.data?.owner);

  const update = useMutation({
    mutationFn: async () => {
      const body: { resolutions: { path: string; field: string; keep: "ours" | "theirs" }[] } = {
        resolutions: Object.entries(resolutions).map(([key, keep]) => {
          const [path, field] = key.split("::");
          return { path, field, keep };
        }),
      };
      return unwrap(
        await api.POST(
          "/api/v1/projects/{project}/workspaces/{name}/update",
          {
            params: { path: { project, name } },
            body,
          },
        ),
      );
    },
    onSuccess: () => {
      setUpdateError(null);
      // The merge moved the base: what was chosen answered the conflicts that are gone now.
      setResolutions({});
      void queryClient.invalidateQueries({
        queryKey: [...queryKeys.list(project, "workspaces"), name, "compare"],
      });
    },
    onError: (err: Error) => setUpdateError(err.message),
  });

  const propose = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST(
          "/api/v1/projects/{project}/workspaces/{name}/propose",
          {
            params: { path: { project, name } },
          },
        ),
      ),
    onSuccess: (data) => {
      const changeId = (data as { metadata?: { name?: string } })?.metadata?.name;
      if (changeId) setProposedChange(changeId);
    },
    onError: (err: Error) => setProposeError(err.message),
  });

  const files = comparison.data?.files ?? [];
  const conflicts = comparison.data?.conflicts ?? [];

  // Determine riskiest lane
  const laneOrder = ["red", "yellow", "green"];
  let riskiestLane: string | undefined;
  for (const file of files) {
    if (file.lane && (!riskiestLane || laneOrder.indexOf(file.lane) < laneOrder.indexOf(riskiestLane))) {
      riskiestLane = file.lane;
    }
  }

  // Count total fields that need resolution
  const totalConflictFields = conflicts.reduce(
    (sum, c) => sum + c.fields.length,
    0,
  );
  const resolvedCount = Object.keys(resolutions).length;
  const allResolved = totalConflictFields === 0 || resolvedCount >= totalConflictFields;

  if (workspace.isPending || comparison.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (workspace.isError || comparison.isError) {
    return (
      <div className="p-4">
        <p className="text-danger">{t("app.error.generic")}</p>
      </div>
    );
  }

  const { title, owner, createdAt } = workspace.data;
  const display = title ?? name;
  const date = new Date(createdAt).toLocaleDateString();

  return (
    <section aria-label={t("workspaces.bringBack.title")} className="space-y-6">
      <PageHeader
        title={t("workspaces.bringBack.title")}
        description={t("workspaces.bringBack.from", { name: display, owner, date })}
      />

      {riskiestLane ? (
        <p className="text-body">
          {t(`workspaces.bringBack.lane.${riskiestLane}`)}
        </p>
      ) : null}

      {proposedChange ? (
        <Alert tone="success" role="status">
          {t("workspaces.bringBack.proposed")}{" "}
          <Link
            to="/projects/$project/approvals/$id"
            params={{ project, id: proposedChange }}
            className="underline"
          >
            {proposedChange}
          </Link>
        </Alert>
      ) : null}

      {proposeError ? (
        <Alert tone="danger" role="alert">
          {proposeError}
        </Alert>
      ) : null}

      {updateError ? (
        <Alert tone="danger" role="alert">
          {updateError}
        </Alert>
      ) : null}

      {!isOwner ? (
        <p className="text-body text-fg-muted">
          {t("workspaces.bringBack.notOwner")}
        </p>
      ) : null}

      {files.length === 0 ? (
        <EmptyState title={t("workspaces.compare.empty")} />
      ) : (
        <FileList files={files} />
      )}

      {conflicts.length > 0 ? (
        <div className="space-y-6">
          <h2 className="text-title font-semibold">
            {t("workspaces.bringBack.conflicts")}
          </h2>
          {conflicts.map((conflict, idx) => (
            <div
              key={`${conflict.path}-${idx}`}
              className="rounded-lg border border-warning/30 bg-warning-soft p-4"
            >
              <p className="font-mono text-sm font-medium mb-2">
                {conflict.path}
              </p>
              {conflict.fields.length === 0 ? (
                <p className="text-sm">{t("workspaces.bringBack.mergeable")}</p>
              ) : (
                <div className="space-y-3">
                  {conflict.fields.map((field) => {
                    const key = `${conflict.path}::${field.path}`;
                    return (
                      <fieldset key={key} className="border-t border-border pt-2">
                        <legend className="text-sm font-medium">
                          {field.path === ""
                            ? t("workspaces.bringBack.wholeFile")
                            : field.path}
                        </legend>
                        <div className="flex items-center gap-4 mt-1">
                          <label className="flex items-center gap-1 text-sm">
                            <input
                              type="radio"
                              name={key}
                              value="ours"
                              checked={resolutions[key] === "ours"}
                              onChange={() =>
                                setResolutions((prev) => ({
                                  ...prev,
                                  [key]: "ours",
                                }))
                              }
                            />
                            {t("workspaces.bringBack.keepOurs")}:{" "}
                            <span className="font-mono">
                              {valueText(field.ours)}
                            </span>
                          </label>
                          <label className="flex items-center gap-1 text-sm">
                            <input
                              type="radio"
                              name={key}
                              value="theirs"
                              checked={resolutions[key] === "theirs"}
                              onChange={() =>
                                setResolutions((prev) => ({
                                  ...prev,
                                  [key]: "theirs",
                                }))
                              }
                            />
                            {t("workspaces.bringBack.takeTheirs")}:{" "}
                            <span className="font-mono">
                              {valueText(field.theirs)}
                            </span>
                          </label>
                        </div>
                      </fieldset>
                    );
                  })}
                </div>
              )}
            </div>
          ))}
        </div>
      ) : null}

      {isOwner ? (
        <div className="flex flex-wrap gap-3">
          <Button
            variant="secondary"
            disabled={!allResolved || update.isPending}
            loading={update.isPending}
            onClick={() => update.mutate()}
          >
            {t("workspaces.bringBack.update")}
          </Button>
          <Button
            variant="primary"
            disabled={conflicts.length > 0 || propose.isPending}
            loading={propose.isPending}
            onClick={() => propose.mutate()}
          >
            {t("workspaces.bringBack.propose")}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
