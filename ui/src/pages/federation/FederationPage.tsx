import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import {
  contextSourceRegistrationSchema,
  contextSourceRegistrationUiSchema,
} from "../../schemas/kinds";
import { Topology, federationGraphKey } from "./Topology";

const PLURAL = "csrs";

/** The form of one registration: the metadata a manifest carries plus its spec. */
export interface RegistrationForm {
  name?: string;
  title?: string;
  [field: string]: unknown;
}

/** The manifest a form produces (MF-01, MF-36). */
export function toEnvelope(project: string, form: RegistrationForm) {
  const { name, title, ...spec } = form;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSourceRegistration",
    metadata: {
      name: name ?? "",
      namespace: project,
      ...(title?.trim() ? { title } : {}),
    },
    spec: prune(spec),
  };
}

/** rjsf leaves an empty object behind for every group the author opened and left alone. */
function prune<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.filter((item) => item !== undefined && item !== "") as T;
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, member] of Object.entries(value)) {
      const cleaned = prune(member);
      const empty =
        cleaned === undefined ||
        cleaned === "" ||
        (cleaned !== null && typeof cleaned === "object" && Object.keys(cleaned).length === 0);
      if (!empty) {
        out[key] = cleaned;
      }
    }
    return out as T;
  }
  return value;
}

/** Where one registration points, for the table. The name of the target, never an address. */
export function targetOf(spec: Record<string, unknown>, external: string): string {
  const reference = spec["endpointRef"] as { name?: unknown } | undefined;
  if (reference && typeof reference.name === "string") {
    return reference.name;
  }
  // An external source is named after the registration everywhere it is shown, because the
  // address is what a card must not carry and provenance names it the same way (UI-27, EP-71).
  return external;
}

/** The entity types a registration claims, for the table. */
export function claimedTypes(spec: Record<string, unknown>): string[] {
  const information = Array.isArray(spec["information"]) ? spec["information"] : [];
  const types = information.flatMap((entry) => {
    const entities = (entry as { entities?: unknown }).entities;
    return Array.isArray(entities)
      ? entities
          .map((entity) => (entity as { type?: unknown }).type)
          .filter((type): type is string => typeof type === "string")
      : [];
  });
  return [...new Set(types)].sort();
}

/** The federation of one project: who reads whose data, as a list and as a graph (UI-27). */
export function FederationPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const [dialogOpen, setDialogOpen] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, PLURAL),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: PLURAL } },
        }),
      ),
  });

  const registrations = useMemo(() => asManifests(list.data?.items ?? []), [list.data]);

  const propose = useMutation({
    mutationFn: async (form: RegistrationForm) => {
      setFormError(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: PLURAL } },
          body: toEnvelope(project, form) as never,
        }),
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, PLURAL) });
      void queryClient.invalidateQueries({ queryKey: federationGraphKey(project) });
    },
    onError: (err: unknown) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  if (list.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (list.isError) {
    const message =
      list.error instanceof ApiError
        ? (list.error.problem?.detail ?? list.error.message)
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <button
          type="button"
          onClick={() => {
            void list.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-bold">{t("federation.title")}</h1>
          <p className="mt-1 max-w-2xl text-sm text-surface-fg/70">{t("federation.lead")}</p>
        </div>
        <PermissionGuard project={project} kind="ContextSourceRegistration" verb="propose">
        <button
          type="button"
          onClick={() => {
            setFormError(null);
            setDialogOpen(true);
          }}
          className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
        >
          {t("federation.add")}
        </button>
        </PermissionGuard>
      </div>

      {change ? <ChangeNotice change={change} project={project} /> : null}

      {registrations.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("federation.empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left text-sm">
            <caption className="sr-only">{t("federation.title")}</caption>
            <thead>
              <tr className="border-b border-border bg-surface-subtle">
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("federation.field.name")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("federation.field.space")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("federation.field.target")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("federation.field.entities")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("federation.field.identity")}
                </th>
              </tr>
            </thead>
            <tbody>
              {registrations.map((registration: Manifest) => {
                const spec = registration.spec;
                const federation = (spec["federation"] ?? {}) as { identity?: unknown };
                const identity =
                  federation.identity === "caller" ? "caller" : "serviceAccount";
                return (
                  <tr
                    key={registration.metadata.name}
                    className="border-b border-border last:border-0"
                  >
                    <td className="px-4 py-2">
                      <span className="font-mono">{registration.metadata.name}</span>
                      {registration.metadata.title ? (
                        <span className="block text-surface-fg/70">
                          {localized(
                            registration.metadata.title,
                            locale,
                            registration.metadata.name,
                          )}
                        </span>
                      ) : null}
                    </td>
                    <td className="px-4 py-2 font-mono">
                      {refName(spec["contextSpaceRef"])}
                    </td>
                    <td className="px-4 py-2 font-mono">
                      {targetOf(spec, registration.metadata.name)}
                    </td>
                    <td className="px-4 py-2">{claimedTypes(spec).join(", ")}</td>
                    <td className="px-4 py-2">{t(`federation.identity.${identity}`)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <Topology project={project} />

      <ResourceFormDialog<RegistrationForm>
        kind="ContextSourceRegistration"
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("federation.add")}
        description={t("federation.dialog.description")}
        schema={contextSourceRegistrationSchema(t)}
        uiSchema={contextSourceRegistrationUiSchema}
        submitLabel={t("federation.propose")}
        disabled={propose.isPending}
        error={formError}
        onSubmit={(form) => propose.mutate(form)}
      />
    </div>
  );
}
