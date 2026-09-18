import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, unwrap } from "../../api/client";
import { PlanDiffViewer } from "../../components/diff/PlanDiffViewer";
import type { FieldChange } from "../../components/diff/PlanDiffViewer";
import { Alert, Badge, EmptyState, PageHeader } from "../../components/ui";
import type { components } from "../../api/schema";

type Comparison = components["schemas"]["Comparison"];
type ChangeFile = components["schemas"]["ChangeFile"];

const OPERATION_ORDER = ["Create", "Update", "Delete"] as const;

function operationOrder(op: string): number {
  return OPERATION_ORDER.indexOf(op as (typeof OPERATION_ORDER)[number]);
}

function fileName(path: string): string {
  const segments = path.split("/");
  const last = segments[segments.length - 1] ?? path;
  return last.replace(/\.yaml$/, "");
}

/** The list of files from a comparison, grouped by kind and ordered by operation. */
export function FileList({ files }: { files: ChangeFile[] }): React.JSX.Element {
  const { t } = useTranslation();
  const grouped: Record<string, ChangeFile[]> = {};
  for (const file of files) {
    const kind = file.kind ?? "unknown";
    if (!grouped[kind]) grouped[kind] = [];
    grouped[kind].push(file);
  }
  const sortedKinds = Object.keys(grouped).sort();
  return (
    <div className="space-y-6">
      {sortedKinds.map((kind) => {
        const kindFiles = grouped[kind].sort(
          (a, b) => operationOrder(a.operation) - operationOrder(b.operation),
        );
        return (
          <section key={kind}>
            <h3 className="text-body font-semibold text-fg mb-2">{kind}</h3>
            <ul className="space-y-3">
              {kindFiles.map((file) => (
                <li
                  key={file.path}
                  className="rounded-lg border border-border bg-surface p-3"
                >
                  <div className="flex flex-wrap items-center gap-2 mb-2">
                    <span className="font-mono text-sm font-medium text-fg">
                      {fileName(file.path)}
                    </span>
                    <Badge tone="neutral" mono>
                      {file.kind}
                    </Badge>
                    <Badge
                      tone={
                        file.lane === "red"
                          ? "danger"
                          : file.lane === "yellow"
                            ? "warning"
                            : "success"
                      }
                    >
                      {t(`lane.${file.lane}`)}
                    </Badge>
                    <Badge
                      tone={
                        file.operation === "Create"
                          ? "success"
                          : file.operation === "Delete"
                            ? "danger"
                            : "info"
                      }
                    >
                      {file.operation === "Create"
                        ? t("approvals.diffAdded")
                        : file.operation === "Delete"
                          ? t("approvals.diffRemoved")
                          : t("approvals.diffChanged")}
                    </Badge>
                  </div>
                  {file.fields && file.fields.length > 0 ? (
                    <details className="text-sm">
                      <summary className="cursor-pointer text-fg-muted hover:text-fg">
                        {t("workspaces.compare.showDiff")}
                      </summary>
                      <div className="mt-2">
                        <PlanDiffViewer fields={file.fields as FieldChange[]} />
                      </div>
                    </details>
                  ) : null}
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

export function ComparePage({
  project,
  name,
}: {
  project: string;
  name: string;
}): React.JSX.Element {
  const { t } = useTranslation();
  const compareQuery = useQuery({
    queryKey: ["workspaces", project, name, "compare"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}/compare", {
          params: { path: { project, name } },
        }),
      ),
  });

  if (compareQuery.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (compareQuery.isError) {
    return (
      <Alert role="alert" tone="danger">
        {t("app.error.generic")}
      </Alert>
    );
  }

  const comparison = compareQuery.data as Comparison;
  const files = comparison.files ?? [];
  const conflicts = comparison.conflicts ?? [];

  const added = files.filter((f) => f.operation === "Create").length;
  const changed = files.filter((f) => f.operation === "Update").length;
  const removed = files.filter((f) => f.operation === "Delete").length;

  return (
    <section aria-label={t("workspaces.compare.title")} className="space-y-6">
      <PageHeader
        title={t("workspaces.compare.title")}
        description={t("workspaces.compare.summary", { added, changed, removed })}
      />
      {conflicts.length > 0 ? (
        <Alert tone="warning" role="status">
          {t("workspaces.compare.conflicts", { count: conflicts.length })}
          <Link
            to="/projects/$project/workspaces/$name/bring-back"
            params={{ project, name }}
            className="ml-2 underline"
          >
            {t("workspaces.compare.resolveConflicts")}
          </Link>
        </Alert>
      ) : null}
      {files.length === 0 ? (
        <EmptyState title={t("workspaces.compare.empty")} />
      ) : (
        <FileList files={files} />
      )}
    </section>
  );
}
