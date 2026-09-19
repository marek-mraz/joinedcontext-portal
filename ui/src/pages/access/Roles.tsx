import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseYaml } from "yaml";
import { manifestName } from "./manifestName";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { asManifests, isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import {
  Alert,
  Button,
  Dialog,
  EmptyState,
  Field,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
  Textarea,
} from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";

interface Rule {
  kinds?: string[];
  verbs?: string[];
  constraints?: { field?: string }[];
}

/** One role of the list, with the namespace it came from so its row can be edited where it lives. */
interface Row {
  name: string;
  home: string;
  rules: Rule[];
}

/** What a new role starts from: the shape of a rule, so nobody has to remember it (PF-49). */
function skeleton(project: string): string {
  return [
    "apiVersion: joinedcontext.com/v1alpha1",
    "kind: Role",
    `metadata: { name: "", namespace: ${project} }`,
    "spec:",
    "  rules:",
    "    - kinds: [Pipeline, DataSource]",
    "      verbs: [propose]",
    "",
  ].join("\n");
}

function useRoles(project: string) {
  return useQuery({
    queryKey: queryKeys.list(project, "roles"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "roles" } },
        }),
      ),
  });
}

/**
 * A role of this project, proposed as a change (PF-68, PF-52): a role written here names the
 * project's kinds and only the verbs its author already holds, which the Portal checks and says
 * plainly when it refuses.
 */
export function NewRoleDialog({
  project,
  open,
  onOpenChange,
}: {
  project: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const [source, setSource] = useState(() => skeleton(project));
  const [invalid, setInvalid] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const propose = useMutation({
    mutationFn: async (manifest: unknown) =>
      proposeChecked(project, "roles", manifest as { metadata: { name: string } }, true),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "roles") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const named = manifestName(source) !== "";

  const submit = () => {
    let manifest: unknown;
    try {
      manifest = parseYaml(source);
    } catch (error) {
      setInvalid(error instanceof Error ? error.message : String(error));
      return;
    }
    setInvalid(null);
    propose.mutate(manifest);
  };

  const close = (next: boolean) => {
    if (!next) {
      setSource(skeleton(project));
      setInvalid(null);
      setChange(null);
      propose.reset();
    }
    onOpenChange(next);
  };

  const failure =
    invalid ??
    (propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null);

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="lg"
      title={t("access.projectRoles.newTitle")}
      description={t("access.projectRoles.newLead", { project })}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={propose.isPending || !named}
              aria-describedby={named ? undefined : `${ids}-name-first`}
              onClick={submit}
            >
              {t("access.projectRoles.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <Field id={`${ids}-source`} label={t("access.projectRoles.sourceLabel")} required>
            <Textarea
              id={`${ids}-source`}
              rows={12}
              spellCheck={false}
              value={source}
              onChange={(event) => {
                setSource(event.target.value);
                setInvalid(null);
              }}
            />
          </Field>
          {named ? null : (
            <p id={`${ids}-name-first`} className="text-sm text-fg-muted">
              {t("access.nameFirst")}
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}

/**
 * Project → Access → the roles in force here: the organization's, which every project shares, and
 * this project's own, which its steward writes and nobody outside it can be given (PF-68, PF-69).
 */
export function Roles({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const organization = useRoles(ORG_NAMESPACE);
  const here = useRoles(project);
  const [writing, setWriting] = useState(false);

  const rows: Row[] = [
    ...asManifests(here.data?.items ?? []).map((role: Manifest) => ({
      name: role.metadata.name,
      home: project,
      rules: ((role.spec ?? {}) as { rules?: Rule[] }).rules ?? [],
    })),
    ...asManifests(organization.data?.items ?? []).map((role: Manifest) => ({
      name: role.metadata.name,
      home: ORG_NAMESPACE,
      rules: ((role.spec ?? {}) as { rules?: Rule[] }).rules ?? [],
    })),
  ];

  /** What a role grants, in one line: `propose, approve on Pipeline, DataSource`. */
  const grants = (rules: Rule[]) =>
    rules
      .map((rule) => `${(rule.verbs ?? []).join(", ")} ${t("access.projectRoles.on")} ${(rule.kinds ?? []).join(", ")}`)
      .join("; ");

  const error = organization.error ?? here.error;
  const pending = organization.isPending || here.isPending;

  return (
    <section className="space-y-4" aria-labelledby="project-roles-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="project-roles-heading" className="text-title font-semibold text-fg">
            {t("access.projectRoles.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("access.projectRoles.lead")}</p>
        </div>
        <PermissionGuard project={project} kind="Role" verb="propose">
          <Button variant="primary" onClick={() => setWriting(true)}>
            {t("access.projectRoles.new")}
          </Button>
        </PermissionGuard>
      </div>

      {error ? (
        <Alert tone="danger" role="alert">
          {error instanceof ApiError ? (error.problem?.detail ?? error.message) : t("app.error.generic")}
        </Alert>
      ) : (
        <Table caption={t("access.projectRoles.caption", { project })} status={pending ? t("app.loading") : undefined}>
          <TableHead>
            <TableHeaderCell>{t("access.projectRoles.role")}</TableHeaderCell>
            <TableHeaderCell>{t("access.projectRoles.where")}</TableHeaderCell>
            <TableHeaderCell>{t("access.projectRoles.grants")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
          {pending ? (
            <TableSkeleton columns={4} />
          ) : (
            <TableBody>
              {rows.length === 0 ? (
                <TableEmpty columns={4}>
                  <EmptyState bare
                    title={t("access.projectRoles.empty")}
                    description={t("access.projectRoles.emptyHint")} />
                </TableEmpty>
              ) : (
                rows.map((row) => (
                  <TableRow key={`${row.home}/${row.name}`}>
                    <TableCell primary>{row.name}</TableCell>
                    <TableCell>
                      {row.home === ORG_NAMESPACE
                        ? t("access.roles.organization")
                        : t("access.roles.project", { name: row.home })}
                    </TableCell>
                    <TableCell>{grants(row.rules)}</TableCell>
                    <TableCell align="right">
                      <span className="inline-flex items-center gap-1.5">
                        <EditResourceAction
                          target={{
                            project,
                            home: row.home,
                            kind: "Role",
                            plural: "roles",
                            name: row.name,
                            label: row.name,
                          }}
                        />
                        <DeleteResourceAction
                          target={{
                            project,
                            home: row.home,
                            kind: "Role",
                            plural: "roles",
                            name: row.name,
                            label: row.name,
                          }}
                        />
                      </span>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          )}
        </Table>
      )}

      <NewRoleDialog project={project} open={writing} onOpenChange={setWriting} />
    </section>
  );
}
