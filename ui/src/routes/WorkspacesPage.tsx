import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../api/client";
import { useAuth } from "../auth/AuthProvider";
import { WorkOnCopyAction } from "../components/WorkOnCopyDialog";
import {
  Badge,
  Button,
  EmptyState,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../components/ui";
import type { components } from "../api/schema";

type WorkspaceView = components["schemas"]["WorkspaceView"];

export function WorkspacesPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const { identity } = useAuth();
  const navigate = useNavigate();
  // Into the copy at the project's spaces; someone else's copy reads, and refuses a write.
  const openIn = (name: string) =>
    void navigate({
      to: "/projects/$project/$plural",
      params: { project, plural: "spaces" },
      search: { workspace: name } as never,
    });
  const queryClient = useQueryClient();

  const list = useQuery({
    queryKey: queryKeys.list(project, "workspaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces", {
          params: { path: { project } },
        }),
      ),
  });

  const discard = useMutation({
    mutationFn: async (name: string) =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/workspaces/{name}", {
          params: { path: { project, name } },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.list(project, "workspaces"),
      });
    },
  });

  const items = list.data?.items ?? [];
  // The owner is who opened it, by the address commits are signed with, or the username.
  const isMine = (w: WorkspaceView) =>
    !!identity && (w.owner === identity.email || w.owner === identity.username);
  const mine = items.filter(isMine);
  const others = items.filter((w) => !isMine(w));

  return (
    <section aria-label={t("workspaces.title")} className="space-y-6">
      <PageHeader
        title={t("workspaces.title")}
        description={t("workspaces.lead")}
        actions={
          <WorkOnCopyAction
            project={project}
            scope={{ kind: "project" }}
            label={t("workspaces.new")}
            variant="primary"
          />
        }
      />

      {list.isPending ? <p role="status">{t("app.loading")}</p> : null}
      {list.isError ? (
        <p className="text-danger">{t("app.error.generic")}</p>
      ) : null}

      {!list.isPending && !list.isError && items.length === 0 ? (
        <EmptyState title={t("workspaces.empty")} />
      ) : (
        <div className="space-y-8">
          {mine.length > 0 ? (
            <section>
              <h2 className="text-title font-semibold mb-3">
                {t("workspaces.mine")}
              </h2>
              <WorkspaceTable
                project={project}
                workspaces={mine}
                isMine
                onOpen={openIn}
                onDiscard={(name) => {
                  if (window.confirm(t("workspaces.discardConfirm", { name }))) {
                    discard.mutate(name);
                  }
                }}
              />
            </section>
          ) : null}
          {others.length > 0 ? (
            <section>
              <h2 className="text-title font-semibold mb-3">
                {t("workspaces.othersTitle")}
              </h2>
              <WorkspaceTable
                project={project}
                workspaces={others}
                isMine={false}
                onOpen={openIn}
              />
            </section>
          ) : null}
        </div>
      )}
    </section>
  );
}

function WorkspaceTable({
  project,
  workspaces,
  isMine,
  onOpen,
  onDiscard,
}: {
  project: string;
  workspaces: WorkspaceView[];
  isMine: boolean;
  onOpen: (name: string) => void;
  onDiscard?: (name: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Table caption={t("workspaces.tableCaption")}>
      <TableHead>
        <TableHeaderCell>{t("workspaces.name")}</TableHeaderCell>
        {!isMine ? (
          <TableHeaderCell>{t("workspaces.owner")}</TableHeaderCell>
        ) : null}
        <TableHeaderCell>{t("workspaces.scope")}</TableHeaderCell>
        <TableHeaderCell>{t("workspaces.expiresColumn")}</TableHeaderCell>
        <TableHeaderCell>{t("workspaces.previewColumn")}</TableHeaderCell>
        <TableHeaderCell>{t("workspaces.actions")}</TableHeaderCell>
      </TableHead>
      <TableBody>
        {workspaces.map((ws) => (
          <TableRow key={ws.name}>
            <TableCell primary>
              <span className="font-medium">{ws.title ?? ws.name}</span>
              {ws.title ? (
                <span className="block text-caption text-fg-muted">
                  {ws.name}
                </span>
              ) : null}
            </TableCell>
            {!isMine ? (
              <TableCell>{ws.owner}</TableCell>
            ) : null}
            <TableCell>
              {ws.scope?.kind === "project"
                ? t("workspaces.scopeProject")
                : ws.scope?.kind === "space"
                  ? t("workspaces.scopeSpace", { name: ws.scope.name })
                  : ws.scope?.kind === "resources"
                    ? ws.scope.items?.map((i) => `${i.kind}/${i.name}`).join(", ") ?? ""
                    : ""}
            </TableCell>
            <TableCell>
              {t("workspaces.expires", {
                date: new Date(ws.expiresAt).toLocaleDateString(),
              })}
            </TableCell>
            <TableCell>
              <Badge
                tone={
                  ws.previewState === "error"
                    ? "danger"
                    : ws.previewState === "running"
                      ? "success"
                      : ws.previewState === "starting"
                        ? "warning"
                        : "neutral"
                }
              >
                {t(`workspaces.previewStates.${ws.previewState}`)}
              </Badge>
            </TableCell>
            <TableCell>
              <div className="flex flex-wrap gap-2">
                <Button size="sm" onClick={() => onOpen(ws.name)}>
                  {t("workspaces.openAction")}
                </Button>
                <Link
                  to="/projects/$project/workspaces/$name/compare"
                  params={{ project, name: ws.name }}
                  className="focus-ring inline-flex items-center rounded-md px-2 py-1 text-sm underline hover:no-underline"
                >
                  {t("workspaces.compareAction")}
                </Link>
                {isMine && onDiscard ? (
                  <Button
                    size="sm"
                    variant="danger"
                    onClick={() => onDiscard(ws.name)}
                  >
                    {t("workspaces.discard")}
                  </Button>
                ) : null}
              </div>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
