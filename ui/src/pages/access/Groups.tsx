import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseYaml } from "yaml";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import {
  Alert,
  Badge,
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

interface Member {
  user?: string;
}

/** One group of the organization, with what the last reconcile said about the realm. */
interface Row {
  name: string;
  description: string;
  members: string[];
  /** The reconciler's word: what it had to correct, or whom the realm does not know yet. */
  sync?: { reason: string; message: string; ok: boolean };
}

/** What a new group starts from, so nobody has to remember the shape (PF-62). */
const SKELETON = [
  "apiVersion: joinedcontext.com/v1alpha1",
  "kind: Group",
  `metadata: { name: city-leads, namespace: ${ORG_NAMESPACE} }`,
  "spec:",
  "  description: The people who lead the city's projects",
  "  members:",
  "    - user: someone@example.org",
  "",
].join("\n");

function useGroups() {
  return useQuery({
    queryKey: queryKeys.list(ORG_NAMESPACE, "groups"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "groups" } },
        }),
      ),
  });
}

/**
 * A group of the organization, proposed as a change (PF-62, PF-52): who is in a group is who a
 * binding names, so a membership is reviewed in the red lane like the binding itself.
 */
export function NewGroupDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const [source, setSource] = useState(SKELETON);
  const [invalid, setInvalid] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const propose = useMutation({
    mutationFn: async (manifest: unknown) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "groups" } },
          body: manifest as never,
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(ORG_NAMESPACE, "groups") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(ORG_NAMESPACE) });
    },
  });

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
      setSource(SKELETON);
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
      title={t("access.groups.newTitle")}
      description={t("access.groups.newLead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button variant="primary" disabled={propose.isPending} onClick={submit}>
              {t("access.groups.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={ORG_NAMESPACE} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <Field id={`${ids}-source`} label={t("access.groups.sourceLabel")} required>
            <Textarea
              id={`${ids}-source`}
              rows={10}
              spellCheck={false}
              value={source}
              onChange={(event) => {
                setSource(event.target.value);
                setInvalid(null);
              }}
            />
          </Field>
        </div>
      )}
    </Dialog>
  );
}

/**
 * Project → Access → the groups of the organization: who is in each one, and what the last
 * reconcile had to correct in Keycloak (PF-62, PF-63). Adding a member is editing the manifest,
 * which is a change somebody approves — never a direct write.
 */
export function Groups({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const groups = useGroups();
  const [writing, setWriting] = useState(false);

  const rows: Row[] = asManifests(groups.data?.items ?? []).map((group: Manifest) => {
    const spec = (group.spec ?? {}) as { description?: string; members?: Member[] };
    const condition = (group.status?.conditions ?? []).find((c) => c.type === "GroupSynced");
    return {
      name: group.metadata.name,
      description: spec.description ?? "",
      members: (spec.members ?? []).map((member) => member.user ?? "").filter(Boolean),
      sync: condition
        ? {
            reason: condition.reason ?? "",
            message: condition.message ?? "",
            ok: condition.status === "True",
          }
        : undefined,
    };
  });

  return (
    <section className="space-y-4" aria-labelledby="groups-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="groups-heading" className="text-title font-semibold text-fg">
            {t("access.groups.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("access.groups.lead")}</p>
        </div>
        {/* The organization is where a group lives, so the right to propose one is read there. */}
        <PermissionGuard project={ORG_NAMESPACE} kind="Group" verb="propose">
          <Button variant="primary" onClick={() => setWriting(true)}>
            {t("access.groups.new")}
          </Button>
        </PermissionGuard>
      </div>

      {groups.error ? (
        <Alert tone="danger" role="alert">
          {groups.error instanceof ApiError
            ? (groups.error.problem?.detail ?? groups.error.message)
            : t("app.error.generic")}
        </Alert>
      ) : (
        <Table
          caption={t("access.groups.caption")}
          status={groups.isPending ? t("app.loading") : undefined}
        >
          <TableHead>
            <TableHeaderCell>{t("access.groups.group")}</TableHeaderCell>
            <TableHeaderCell>{t("access.groups.members")}</TableHeaderCell>
            <TableHeaderCell>{t("access.groups.sync")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
          {groups.isPending ? (
            <TableSkeleton columns={4} />
          ) : (
            <TableBody>
              {rows.length === 0 ? (
                <TableEmpty columns={4}>
                  <EmptyState bare title={t("access.groups.empty")} />
                </TableEmpty>
              ) : (
                rows.map((row) => (
                  <TableRow key={row.name}>
                    <TableCell primary>
                      <div>{row.name}</div>
                      {row.description ? (
                        <div className="mt-0.5 text-caption text-fg-subtle">{row.description}</div>
                      ) : null}
                    </TableCell>
                    <TableCell>
                      {row.members.length === 0 ? (
                        <span className="text-fg-subtle">{t("access.groups.nobody")}</span>
                      ) : (
                        <ul className="flex flex-col gap-0.5">
                          {row.members.map((member) => (
                            <li key={member} className="font-mono text-caption">
                              {member}
                            </li>
                          ))}
                        </ul>
                      )}
                    </TableCell>
                    <TableCell>
                      {row.sync ? (
                        <div className="flex flex-col gap-1">
                          <Badge tone={row.sync.ok ? "warning" : "danger"}>
                            {t(`access.groups.reason.${row.sync.reason}`, {
                              defaultValue: row.sync.reason,
                            })}
                          </Badge>
                          <span className="text-caption text-fg-muted">{row.sync.message}</span>
                        </div>
                      ) : (
                        <span className="text-fg-subtle">{t("access.groups.inStep")}</span>
                      )}
                    </TableCell>
                    <TableCell align="right">
                      <span className="inline-flex items-center gap-1.5">
                        {/* Adding or removing a member is editing the manifest (PF-52). */}
                        <EditResourceAction
                          target={{
                            project,
                            home: ORG_NAMESPACE,
                            kind: "Group",
                            plural: "groups",
                            name: row.name,
                            label: row.name,
                          }}
                        />
                        <DeleteResourceAction
                          target={{
                            project,
                            home: ORG_NAMESPACE,
                            kind: "Group",
                            plural: "groups",
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

      <NewGroupDialog open={writing} onOpenChange={setWriting} />
    </section>
  );
}
