import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { readsOf, writesOf } from "../../pages/access/EffectivePermissions";
import type { FilterSlot } from "./filters";

/** One entry of the gateway's AuthZEN permissions document (EP-55, EP-56). */
export interface GrantEntry {
  resource?: { type?: string; id?: string; idPatterns?: string[] };
  actions?: string[];
  attributes?: string[] | "*";
  constraints?: { q?: string; scopeQ?: string; geoQ?: string; temporalQ?: string };
}

export interface GrantDocument {
  subject?: { type?: string; id?: string };
  permissions?: GrantEntry[];
  prohibitions?: GrantEntry[];
}

/** The two decisions worth knowing before a query: a read, and the write a pipeline would do. */
export const CHECKED_ACTIONS = ["queryEntity", "updateAttrs"] as const;

const covers = (entry: GrantEntry, type: string) =>
  entry.resource?.type === type || entry.resource?.type === "*";

/**
 * The attributes of `slots` the caller may not read for `type`, each with the reason: named
 * by a prohibition, or outside every grant's attribute list. A grant with `*` denies nothing.
 */
export function deniedAttributes(
  doc: GrantDocument | undefined,
  type: string | undefined,
  slots: FilterSlot[],
  t: (key: string) => string,
): Record<string, string> {
  if (!doc || !type) {
    return {};
  }
  const denied: Record<string, string> = {};
  const grants = (doc.permissions ?? []).filter((entry) => covers(entry, type));
  const unlimited = grants.some((entry) => entry.attributes === "*" || entry.attributes === undefined);
  const granted = new Set(grants.flatMap((entry) => (Array.isArray(entry.attributes) ? entry.attributes : [])));
  if (grants.length > 0 && !unlimited) {
    for (const slot of slots) {
      if (!granted.has(slot.name)) {
        denied[slot.name] = t("access.panel.attrDenied");
      }
    }
  }
  for (const entry of (doc.prohibitions ?? []).filter((e) => covers(e, type))) {
    for (const attribute of Array.isArray(entry.attributes) ? entry.attributes : []) {
      denied[attribute] = t("access.panel.attrProhibited");
    }
  }
  return denied;
}

async function gatewayFetch(slug: string, path: string, init?: RequestInit): Promise<Response> {
  // A `Request` rather than a URL string, as `api/client.ts` sends: same origin, the user's session.
  const response = await globalThis.fetch(
    new Request(new URL(`/api/endpoint/${encodeURIComponent(slug)}${path}`, window.location.origin), {
      credentials: "same-origin",
      ...init,
      headers: { Accept: "application/json", ...(init?.headers ?? {}) },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, `HTTP ${response.status}`);
  }
  return response;
}

/** The caller's grants on one endpoint, read with their session and kept for this page only. */
export function useAccess(slug: string | undefined) {
  return useQuery({
    queryKey: ["access", slug],
    enabled: Boolean(slug),
    retry: false,
    // Nothing of one person's grants outlives their page (R17): no reuse across a sign-in.
    gcTime: 0,
    queryFn: async () => (await (await gatewayFetch(slug!, "/access")).json()) as GrantDocument,
  });
}

/**
 * "Your access" (T-0529, UI-33, R51): what the endpoint's own PDP says the caller may do with
 * the chosen type, and the dry-run decisions for the current query, before it runs.
 */
export function AccessPanel({
  slug,
  type,
  access,
}: {
  slug: string | undefined;
  type: string | undefined;
  access: ReturnType<typeof useAccess>;
}): JSX.Element | null {
  const { t } = useTranslation();
  const checks = useQuery({
    queryKey: ["access-check", slug, type],
    enabled: Boolean(slug && type),
    retry: false,
    gcTime: 0,
    queryFn: async () =>
      Promise.all(
        CHECKED_ACTIONS.map(async (action) => {
          const response = await gatewayFetch(slug!, "/access/check", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ action: { name: action }, resource: { type } }),
          });
          const body = (await response.json()) as { decision?: boolean };
          return { action, decision: body.decision === true };
        }),
      ),
  });

  if (!slug) {
    return null;
  }
  const entries = (access.data?.permissions ?? []).filter((entry) => !type || covers(entry, type));
  const prohibited = (access.data?.prohibitions ?? []).filter((entry) => !type || covers(entry, type));

  return (
    <section aria-labelledby="access-panel" className="rounded-md border border-border bg-surface p-3 text-caption">
      <h2 id="access-panel" className="text-body font-semibold text-fg">
        {t("access.panel.title")}
      </h2>
      {access.isError ? (
        <p role="status" className="text-fg-muted">
          {access.error instanceof ApiError && access.error.status === 403
            ? t("access.matrix.forbidden")
            : t("access.matrix.unavailable")}
        </p>
      ) : null}
      {access.data && !type ? <p className="text-fg-muted">{t("access.panel.chooseType")}</p> : null}
      {access.data && type ? (
        <div className="flex flex-col gap-2">
          {entries.length === 0 ? (
            <p className="text-fg-muted">{t("access.panel.typeNotGranted", { type })}</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {entries.map((entry, index) => (
                <li key={index} className="flex flex-wrap items-center gap-2">
                  <span className="font-mono">{entry.resource?.type}</span>
                  <span>
                    {t("access.matrix.read")}: {readsOf(entry.actions ?? []).join(", ") || t("access.matrix.none")}
                  </span>
                  <span>
                    {t("access.matrix.write")}: {writesOf(entry.actions ?? []).join(", ") || t("access.matrix.none")}
                  </span>
                  <span>
                    {t("access.matrix.attributes")}:{" "}
                    {Array.isArray(entry.attributes) ? entry.attributes.join(", ") : t("access.matrix.allAttributes")}
                  </span>
                  {Object.entries(entry.constraints ?? {})
                    .filter(([, value]) => Boolean(value))
                    .map(([name, value]) => (
                      <span key={name} className="font-mono text-fg-muted">
                        {name}={value}
                      </span>
                    ))}
                </li>
              ))}
            </ul>
          )}
          {prohibited.length > 0 ? (
            <p className="text-fg-muted">
              {t("access.matrix.prohibitions")}:{" "}
              {prohibited
                .map((entry) => `${entry.resource?.type ?? "*"} ${(entry.actions ?? []).join(", ")}`)
                .join("; ")}
            </p>
          ) : null}
          <ul className="flex flex-wrap gap-2" aria-label={t("access.panel.checks")}>
            {(checks.data ?? CHECKED_ACTIONS.map((action) => ({ action, decision: undefined }))).map((check) => (
              <li
                key={check.action}
                className={`rounded border px-2 py-0.5 font-mono ${
                  check.decision === undefined
                    ? "border-border text-fg-muted"
                    : check.decision
                      ? "border-success bg-success/10 text-success"
                      : "border-danger bg-danger/10 text-danger"
                }`}
              >
                {check.decision === undefined
                  ? t("access.panel.checking", { action: check.action })
                  : check.decision
                    ? t("access.panel.may", { action: check.action, type })
                    : t("access.panel.mayNot", { action: check.action, type })}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </section>
  );
}
