import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";

/**
 * The AuthZEN grant document as the Context Gateway serves it today (EP-55, EP-56): one entry
 * per entity type a policy names, with the operations, the attribute projection and the residual
 * the gateway would add to any request. A type no grant names is absent, never listed as denied
 * (EP-59), so an empty matrix means "nothing here for you", not "nothing exists".
 */
interface GrantEntry {
  resource?: { type?: string; id?: string; idPatterns?: string[] };
  actions?: string[];
  attributes?: string[] | "*";
  constraints?: { q?: string; scopeQ?: string; geoQ?: string; temporalQ?: string };
}

interface GrantDocument {
  subject?: { type?: string; id?: string };
  resource?: { type?: string; id?: string; space?: string };
  permissions?: GrantEntry[];
  prohibitions?: GrantEntry[];
}

/** CIM 009 operation names that write; everything else on an entry is a read (R8). */
const WRITE_OPERATIONS = new Set([
  "createEntity",
  "updateEntity",
  "appendAttrs",
  "updateAttrs",
  "deleteAttrs",
  "deleteEntity",
  "mergeEntity",
  "replaceEntity",
  "replaceAttrs",
  "batchOps",
  "upsertBatch",
  "createBatch",
  "updateBatch",
  "deleteBatch",
]);

export function writesOf(actions: string[]): string[] {
  return actions.filter((action) => WRITE_OPERATIONS.has(action));
}

export function readsOf(actions: string[]): string[] {
  return actions.filter((action) => !WRITE_OPERATIONS.has(action));
}

function ConstraintList({ entry }: { entry: GrantEntry }): JSX.Element {
  const { t } = useTranslation();
  const residual = Object.entries(entry.constraints ?? {}).filter(([, value]) => Boolean(value));
  if (residual.length === 0) {
    return <span className="text-sm text-surface-fg/60">{t("access.matrix.unconstrained")}</span>;
  }
  return (
    <ul className="space-y-1">
      {residual.map(([name, value]) => (
        <li key={name} className="font-mono text-xs">
          <span className="text-surface-fg/60">{name}=</span>
          {value}
        </li>
      ))}
    </ul>
  );
}

function GrantTable({ entries, caption }: { entries: GrantEntry[]; caption: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="overflow-x-auto rounded border border-border">
      <table className="w-full border-collapse text-left text-sm">
        <caption className="sr-only">{caption}</caption>
        <thead>
          <tr className="border-b border-border bg-surface-subtle">
            <th scope="col" className="px-4 py-2 font-medium">
              {t("access.matrix.type")}
            </th>
            <th scope="col" className="px-4 py-2 font-medium">
              {t("access.matrix.read")}
            </th>
            <th scope="col" className="px-4 py-2 font-medium">
              {t("access.matrix.write")}
            </th>
            <th scope="col" className="px-4 py-2 font-medium">
              {t("access.matrix.attributes")}
            </th>
            <th scope="col" className="px-4 py-2 font-medium">
              {t("access.matrix.residual")}
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {entries.map((entry, index) => {
            const actions = entry.actions ?? [];
            const reads = readsOf(actions);
            const writes = writesOf(actions);
            const attributes = entry.attributes;
            return (
              <tr key={`${entry.resource?.type ?? "?"}-${index}`}>
                <th scope="row" className="px-4 py-3 text-left font-medium">
                  {entry.resource?.type ?? "?"}
                  {entry.resource?.idPatterns?.length ? (
                    <div className="font-mono text-xs font-normal text-surface-fg/60">
                      {entry.resource.idPatterns.join(", ")}
                    </div>
                  ) : null}
                </th>
                <td className="px-4 py-3">
                  {reads.length === 0 ? (
                    <span className="text-surface-fg/40">{t("access.matrix.none")}</span>
                  ) : (
                    <ul className="flex flex-wrap gap-1">
                      {reads.map((action) => (
                        <li
                          key={action}
                          className="inline-flex items-center rounded border border-border px-2 py-0.5 font-mono text-xs"
                        >
                          {action}
                        </li>
                      ))}
                    </ul>
                  )}
                </td>
                <td className="px-4 py-3">
                  {writes.length === 0 ? (
                    <span className="text-surface-fg/40">{t("access.matrix.none")}</span>
                  ) : (
                    <ul className="flex flex-wrap gap-1">
                      {writes.map((action) => (
                        <li
                          key={action}
                          className="inline-flex items-center rounded border border-amber-500/40 bg-amber-500/20 px-2 py-0.5 font-mono text-xs"
                        >
                          {action}
                        </li>
                      ))}
                    </ul>
                  )}
                </td>
                <td className="px-4 py-3">
                  {attributes === "*" || attributes === undefined ? (
                    <span className="text-sm">{t("access.matrix.allAttributes")}</span>
                  ) : (
                    <ul className="flex flex-wrap gap-1">
                      {attributes.map((attribute) => (
                        <li key={attribute} className="font-mono text-xs">
                          {attribute}
                        </li>
                      ))}
                    </ul>
                  )}
                </td>
                <td className="px-4 py-3">
                  <ConstraintList entry={entry} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/**
 * What the signed-in person may do through one endpoint, read from that endpoint's own `/access`
 * surface. The Portal never computes this a second time: the document comes from the PDP that
 * enforces the requests, so the table and the gateway can never disagree (EP-60).
 */
export function EffectivePermissions({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [slug, setSlug] = useState<string | null>(null);

  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const published = asManifests(endpoints.data?.items ?? []).filter(
    (endpoint) => typeof (endpoint.spec as { slug?: string }).slug === "string",
  );
  const selected = slug ?? (published[0]?.spec as { slug?: string } | undefined)?.slug ?? null;

  const grants = useQuery({
    queryKey: ["access", project, selected],
    enabled: selected !== null,
    retry: false,
    queryFn: async (): Promise<GrantDocument> => {
      // The gateway's own surface, not a Portal route: one document, one implementation (EP-60).
      const response = await fetch(`/api/endpoint/${encodeURIComponent(selected ?? "")}/access`, {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
      if (!response.ok) {
        throw new ApiError(response.status, `HTTP ${response.status}`);
      }
      return (await response.json()) as GrantDocument;
    },
  });

  return (
    <section className="space-y-4" aria-labelledby="effective-permissions-heading">
      <h2 id="effective-permissions-heading" className="text-lg font-bold">
        {t("access.matrix.title")}
      </h2>
      <p className="text-sm text-surface-fg/70">{t("access.matrix.hint")}</p>

      {published.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("access.matrix.noEndpoint")}</p>
      ) : (
        <label className="block text-sm font-medium">
          {t("access.matrix.endpoint")}
          <select
            value={selected ?? ""}
            onChange={(event) => setSlug(event.target.value)}
            className="mt-1 block w-full max-w-md rounded border border-border bg-surface px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {published.map((endpoint) => (
              <option
                key={endpoint.metadata.name}
                value={(endpoint.spec as { slug?: string }).slug}
              >
                {localized(endpoint.metadata.title, locale, endpoint.metadata.name)}
              </option>
            ))}
          </select>
        </label>
      )}

      {grants.isError ? (
        <p role="status" className="text-sm text-surface-fg/70">
          {grants.error instanceof ApiError && grants.error.status === 403
            ? t("access.matrix.forbidden")
            : t("access.matrix.unavailable")}
        </p>
      ) : null}

      {grants.isPending && selected !== null ? <p role="status">{t("app.loading")}</p> : null}

      {grants.data ? (
        <>
          <p className="text-sm">
            {t("access.matrix.subject", {
              subject: grants.data.subject?.id ?? t("access.matrix.anonymous"),
              space: grants.data.resource?.space ?? "",
            })}
          </p>
          {(grants.data.permissions ?? []).length === 0 ? (
            <p className="text-sm text-surface-fg/70">{t("access.matrix.empty")}</p>
          ) : (
            <GrantTable
              entries={grants.data.permissions ?? []}
              caption={t("access.matrix.title")}
            />
          )}
          {(grants.data.prohibitions ?? []).length > 0 ? (
            <>
              <h3 className="text-base font-medium">{t("access.matrix.prohibitions")}</h3>
              <GrantTable
                entries={grants.data.prohibitions ?? []}
                caption={t("access.matrix.prohibitions")}
              />
            </>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
