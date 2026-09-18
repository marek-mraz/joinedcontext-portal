import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { api, ApiError, unwrap } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Button } from "../../components/ui/Button";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { Field } from "../../components/ui/Field";
import { Textarea } from "../../components/ui/Input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui/Table";

type ProfileAccess = components["schemas"]["ProfileAccess"];

/**
 * What each agent profile lets a run you start do (UI-56, AG-70): per operation the profile's
 * half, your own permission and the reason one refuses. An edit of the access block is a
 * Change on the profile, reviewed like any manifest (CC-03).
 */
export function AgentAccess({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const access = useQuery({
    queryKey: ["projects", project, "assistant", "access"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/assistant/access", {
          params: { path: { project } },
        }),
      ),
  });

  return (
    <section
      aria-labelledby="agent-access-heading"
      className="space-y-4 rounded-lg border border-border bg-surface p-5 shadow-1"
    >
      <div className="space-y-1">
        <h2 id="agent-access-heading" className="text-lg font-semibold text-fg">
          {t("assistantPage.access.title")}
        </h2>
        <p className="text-caption text-fg-muted">{t("assistantPage.access.lead")}</p>
      </div>
      {access.isError ? (
        <p role="alert" className="text-caption text-danger">
          {t("app.error.generic")}
        </p>
      ) : access.data?.items.length === 0 ? (
        <p className="text-caption text-fg-muted">{t("assistantPage.access.none")}</p>
      ) : (
        access.data?.items.map((profile) => (
          <ProfileCard key={profile.name} project={project} profile={profile} />
        ))
      )}
    </section>
  );
}

function ProfileCard({ project, profile }: { project: string; profile: ProfileAccess }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const propose = useMutation({
    mutationFn: async (block: unknown) => {
      const path = { project: ORG_NAMESPACE, plural: "agentprofiles", name: profile.name };
      const current = (await unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path } }),
      )) as unknown as { spec: Record<string, unknown>; status?: unknown };
      const manifest: Record<string, unknown> = { ...current, spec: { ...current.spec } };
      delete manifest.status;
      const spec = manifest.spec as Record<string, unknown>;
      if (block === null || block === undefined) {
        delete spec.access;
      } else {
        spec.access = block;
      }
      return proposeChecked(
        path.project,
        path.plural,
        manifest as { metadata: { name: string } },
        false,
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setText(null);
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "assistant", "access"] });
    },
    onError: (err) => {
      setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : String(err));
    },
  });

  function submit() {
    setError(null);
    let block: unknown;
    try {
      block = parseYaml(text ?? "");
    } catch (err) {
      setError(t("assistantPage.access.yamlError", { message: err instanceof Error ? err.message : String(err) }));
      return;
    }
    propose.mutate(block);
  }

  const available = profile.operations.filter((op) => op.profile && op.person).length;
  const heading = `access-${profile.name}`;

  return (
    <article aria-labelledby={heading} className="space-y-3 rounded-md border border-border p-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3 id={heading} className="font-semibold text-fg">
          {profile.title ?? profile.name}{" "}
          <span className="font-mono text-caption text-fg-muted">
            {profile.name} · {profile.role}
          </span>
        </h3>
        <span className="text-caption text-fg-muted">
          {t("assistantPage.access.available", { count: available, total: profile.operations.length })}
        </span>
      </div>
      <p className="text-caption text-fg-muted">
        {profile.access ? t("assistantPage.access.declared") : t("assistantPage.access.readOnlyDefault")}
      </p>
      <p className="text-caption">
        <span className="font-medium">{t("assistantPage.access.egress")}: </span>
        {profile.egressHosts.length > 0 ? profile.egressHosts.join(", ") : t("assistantPage.access.noEgress")}
      </p>

      <Table caption={t("assistantPage.access.caption", { name: profile.name })}>
        <TableHead>
          <TableHeaderCell>{t("assistantPage.access.operation")}</TableHeaderCell>
          <TableHeaderCell>{t("assistantPage.access.profile")}</TableHeaderCell>
          <TableHeaderCell>{t("assistantPage.access.you")}</TableHeaderCell>
          <TableHeaderCell>{t("assistantPage.access.reason")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {profile.operations.map((op) => (
            <TableRow key={op.name}>
              <TableCell className="font-mono text-caption">{op.name}</TableCell>
              <TableCell>{op.profile ? t("assistantPage.access.yes") : t("assistantPage.access.no")}</TableCell>
              <TableCell>{op.person ? t("assistantPage.access.yes") : t("assistantPage.access.no")}</TableCell>
              <TableCell className="text-caption text-fg-muted">{op.reason ?? ""}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}

      {text === null ? (
        // The block is proposed as the org's `AgentProfile`, so that is the permission the editor
        // needs — not one in the project being read (T-1584, UI-44).
        <PermissionGuard project={ORG_NAMESPACE} kind="AgentProfile" verb="propose">
          <Button
            size="sm"
            onClick={() => {
              setChange(null);
              setText(profile.access ? stringifyYaml(profile.access) : "");
            }}
          >
            {t("assistantPage.access.edit")}
          </Button>
        </PermissionGuard>
      ) : (
        <form
          className="space-y-2"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <Field
            id={`access-yaml-${profile.name}`}
            label={t("assistantPage.access.block")}
            help={t("assistantPage.access.blockHint")}
          >
            <Textarea
              id={`access-yaml-${profile.name}`}
              rows={8}
              className="font-mono"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
          </Field>
          {error ? (
            <p role="alert" className="text-caption font-medium text-danger">
              {error}
            </p>
          ) : null}
          <div className="flex gap-2">
            <PermissionGuard project={ORG_NAMESPACE} kind="AgentProfile" verb="propose">
              <Button type="submit" variant="primary" size="sm" loading={propose.isPending}>
                {t("assistantPage.access.propose")}
              </Button>
            </PermissionGuard>
            <Button size="sm" onClick={() => setText(null)}>
              {t("assistantPage.access.cancel")}
            </Button>
          </div>
        </form>
      )}
    </article>
  );
}
