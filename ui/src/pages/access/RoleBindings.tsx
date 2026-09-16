import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { takePrefill } from "../../assistant/state";
import { useBranding } from "../../branding";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { dns1123 } from "../../components/endpoints/sharing";
import {
  Alert,
  Button,
  Dialog,
  EmptyState,
  Field,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";

interface Scope {
  organization?: string;
  project?: string;
  contextSpace?: string;
}

interface BindingSpec {
  subjects?: { user?: string; group?: string }[];
  role?: string;
  scope?: Scope;
  validity?: { notAfter?: string };
}

/** Where a grant applies, as the form holds it: `organization`, `project`, or `space:<name>`. */
type Place = string;

interface GrantForm {
  subjectKind: "user" | "group";
  subject: string;
  role: string;
  place: Place;
  until: string;
}

function placeOf(scope: Scope | undefined, project: string): Place {
  if (scope?.contextSpace) {
    return `space:${scope.contextSpace}`;
  }
  return scope?.project === project ? "project" : "organization";
}

/** The RoleBinding a filled form asks for; an organization scope names the domain's first label. */
export function bindingOf(form: GrantForm, project: string, orgDomain: string): Manifest {
  const organization = orgDomain.split(".")[0] || ORG_NAMESPACE;
  const scope: Scope =
    form.place === "organization"
      ? { organization }
      : form.place === "project"
        ? { project }
        : { contextSpace: form.place.slice("space:".length) };
  const where = scope.organization ?? scope.project ?? scope.contextSpace ?? "";
  const who = form.subject.trim();
  const spec: BindingSpec = {
    subjects: [form.subjectKind === "user" ? { user: who } : { group: who }],
    role: form.role,
    scope,
  };
  if (form.until) {
    spec.validity = { notAfter: new Date(`${form.until}T23:59:59Z`).toISOString() };
  }
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "RoleBinding",
    metadata: { name: dns1123(who.split("@")[0], form.role, where), namespace: ORG_NAMESPACE },
    spec,
  } as Manifest;
}

/** The form a manifest fills: the assistant's draft, or an empty one. */
function formOf(manifest: Record<string, unknown> | null, project: string): GrantForm {
  const spec = (manifest?.spec ?? {}) as BindingSpec;
  const subject = spec.subjects?.[0];
  return {
    subjectKind: subject?.group ? "group" : "user",
    subject: subject?.group ?? subject?.user ?? "",
    role: spec.role ?? "",
    place: manifest ? placeOf(spec.scope, project) : "project",
    until: spec.validity?.notAfter?.slice(0, 10) ?? "",
  };
}

function useList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
}

/**
 * Giving people or a group a role (AG-77, PF-52): who, which role, where and until when, proposed
 * as a red change. The Portal refuses a grant of anything its proposer does not hold there, and
 * that refusal is shown as it comes.
 */
export function GrantRoleDialog({
  project,
  open,
  onOpenChange,
  prefill,
}: {
  project: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The binding the assistant drafted, when the page was opened on one. */
  prefill: Record<string, unknown> | null;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const branding = useBranding();
  const [form, setForm] = useState<GrantForm>(() => formOf(prefill, project));
  const [change, setChange] = useState<Change | null>(null);
  const roles = useList(ORG_NAMESPACE, "roles");
  const spaces = useList(project, "spaces");

  const propose = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "rolebindings" } },
          body: bindingOf(form, project, branding.orgDomain) as never,
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setForm(formOf(null, project));
      setChange(null);
      propose.reset();
    }
    onOpenChange(next);
  };
  const set = (patch: Partial<GrantForm>) => {
    setForm((current) => ({ ...current, ...patch }));
    propose.reset();
  };

  const roleNames = asManifests(roles.data?.items ?? []).map((role) => role.metadata.name);
  const spaceNames = asManifests(spaces.data?.items ?? []).map((space) => space.metadata.name);
  const failure =
    propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null;
  const ready = form.subject.trim() !== "" && form.role !== "" && !propose.isPending;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("access.roles.grantTitle")}
      description={t("access.roles.grantLead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button variant="primary" disabled={!ready} onClick={() => propose.mutate()}>
              {t("access.roles.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          <Field id={`${ids}-kind`} label={t("access.roles.subjectKind")}>
            <Select
              id={`${ids}-kind`}
              value={form.subjectKind}
              onChange={(event) => set({ subjectKind: event.target.value as GrantForm["subjectKind"] })}
            >
              <option value="user">{t("access.roles.person")}</option>
              <option value="group">{t("access.roles.groupOption")}</option>
            </Select>
          </Field>
          <Field
            id={`${ids}-subject`}
            label={form.subjectKind === "user" ? t("access.roles.personLabel") : t("access.roles.groupLabel")}
            required
          >
            <Input
              id={`${ids}-subject`}
              value={form.subject}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => set({ subject: event.target.value })}
            />
          </Field>
          <Field id={`${ids}-role`} label={t("access.roles.roleLabel")} required>
            <Select id={`${ids}-role`} value={form.role} onChange={(event) => set({ role: event.target.value })}>
              <option value="">{t("access.roles.chooseRole")}</option>
              {roleNames.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
          <Field id={`${ids}-place`} label={t("access.roles.whereLabel")}>
            <Select id={`${ids}-place`} value={form.place} onChange={(event) => set({ place: event.target.value })}>
              <option value="project">{t("access.roles.project", { name: project })}</option>
              <option value="organization">{t("access.roles.organization")}</option>
              {spaceNames.map((name) => (
                <option key={name} value={`space:${name}`}>
                  {t("access.roles.space", { name })}
                </option>
              ))}
            </Select>
          </Field>
          <Field id={`${ids}-until`} label={t("access.roles.untilLabel")}>
            <Input
              id={`${ids}-until`}
              type="date"
              value={form.until}
              onChange={(event) => set({ until: event.target.value })}
            />
          </Field>
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

/** Project → Access → who holds which role here: over the organization, this project or one of its spaces. */
export function RoleBindings({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const bindings = useList(ORG_NAMESPACE, "rolebindings");
  const spaces = useList(project, "spaces");
  const [prefill] = useState(() =>
    typeof window !== "undefined" && new URLSearchParams(window.location.search).get("grant")
      ? takePrefill(window.location.pathname)
      : null,
  );
  const [granting, setGranting] = useState(prefill !== null);

  const spaceNames = new Set(asManifests(spaces.data?.items ?? []).map((space) => space.metadata.name));
  const here = asManifests(bindings.data?.items ?? []).filter((binding) => {
    const scope = (binding.spec as BindingSpec).scope;
    return (
      Boolean(scope?.organization) ||
      scope?.project === project ||
      (scope?.contextSpace !== undefined && spaceNames.has(scope.contextSpace))
    );
  });

  const where = (scope: Scope | undefined) =>
    scope?.contextSpace
      ? t("access.roles.space", { name: scope.contextSpace })
      : scope?.project
        ? t("access.roles.project", { name: scope.project })
        : t("access.roles.organization");
  const who = (spec: BindingSpec) =>
    (spec.subjects ?? [])
      .map((subject) => (subject.group ? t("access.roles.group", { name: subject.group }) : (subject.user ?? "")))
      .join(", ");

  return (
    <section className="space-y-4" aria-labelledby="role-bindings-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="role-bindings-heading" className="text-title font-semibold text-fg">
            {t("access.roles.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("access.roles.lead")}</p>
        </div>
        <PermissionGuard project={ORG_NAMESPACE} kind="RoleBinding" verb="propose">
          <Button variant="primary" onClick={() => setGranting(true)}>
            {t("access.roles.grant")}
          </Button>
        </PermissionGuard>
      </div>

      {bindings.isError ? (
        <Alert tone="danger" role="alert">
          {bindings.error instanceof ApiError
            ? (bindings.error.problem?.detail ?? bindings.error.message)
            : t("app.error.generic")}
        </Alert>
      ) : (
        <Table
          caption={t("access.roles.caption", { project })}
          status={bindings.isPending ? t("app.loading") : undefined}
        >
          <TableHead>
            <TableHeaderCell>{t("access.roles.who")}</TableHeaderCell>
            <TableHeaderCell>{t("access.roles.role")}</TableHeaderCell>
            <TableHeaderCell>{t("access.roles.where")}</TableHeaderCell>
            <TableHeaderCell>{t("access.roles.until")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
          {bindings.isPending ? (
            <TableSkeleton columns={5} />
          ) : (
            <TableBody>
              {here.length === 0 ? (
                <TableEmpty columns={5}>
                  <EmptyState bare title={t("access.roles.empty")} />
                </TableEmpty>
              ) : (
                here.map((binding) => {
                  const spec = binding.spec as BindingSpec;
                  const target = {
                    project,
                    home: ORG_NAMESPACE,
                    kind: "RoleBinding",
                    plural: "rolebindings",
                    name: binding.metadata.name,
                    label: `${who(spec)}: ${spec.role ?? ""}`,
                  };
                  const until = spec.validity?.notAfter;
                  return (
                    <TableRow key={binding.metadata.name}>
                      <TableCell primary>{who(spec)}</TableCell>
                      <TableCell>{spec.role}</TableCell>
                      <TableCell>{where(spec.scope)}</TableCell>
                      <TableCell>
                        {until ? new Date(until).toLocaleDateString(locale) : t("access.roles.noEnd")}
                      </TableCell>
                      <TableCell align="right">
                        <span className="inline-flex items-center gap-1.5">
                          <EditResourceAction target={target} />
                          <DeleteResourceAction target={target} />
                        </span>
                      </TableCell>
                    </TableRow>
                  );
                })
              )}
            </TableBody>
          )}
        </Table>
      )}

      <GrantRoleDialog project={project} open={granting} onOpenChange={setGranting} prefill={prefill} />
    </section>
  );
}
