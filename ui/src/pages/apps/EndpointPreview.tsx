import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { EndpointLink, ENDPOINT_LINKS, endpointUrl } from "../../components/endpoints/links";
import { useAccess } from "../../components/entities/AccessPanel";
import type { GrantDocument } from "../../components/entities/AccessPanel";
import { writesOf } from "../access/EffectivePermissions";
import { fetchJson } from "../endpoints/SchemaProjectionPanel";

/** How many entities are worth showing to judge an endpoint by: enough to see the shape (AP-51). */
const SAMPLE_LIMIT = 5;

/**
 * The endpoint's `/access` document as a few sentences: which types, which attributes, and
 * whether the caller's own grant reaches further than a generated app ever will.
 *
 * A grant with no `attributes` and one with `"*"` mean the same thing, every attribute the
 * endpoint publishes, and neither is a list to print (EP-55).
 */
export function accessWords(document: GrantDocument | undefined): {
  types: string[];
  attrs: string[] | "*";
  writes: string[];
  denied: string[];
} {
  const permissions = document?.permissions ?? [];
  const unlimited = permissions.some(
    (entry) => entry.attributes === "*" || entry.attributes === undefined,
  );
  const named = (entries: { attributes?: string[] | "*" }[]) => [
    ...new Set(entries.flatMap((entry) => (Array.isArray(entry.attributes) ? entry.attributes : []))),
  ].sort();
  return {
    types: [
      ...new Set(
        permissions
          .map((entry) => entry.resource?.type)
          .filter((type): type is string => typeof type === "string"),
      ),
    ].sort(),
    attrs: unlimited ? "*" : named(permissions),
    writes: [...new Set(permissions.flatMap((entry) => writesOf(entry.actions ?? [])))].sort(),
    denied: named(document?.prohibitions ?? []),
  };
}

/** One served entity as one line: the id, the type, and the first few values it carries. */
function sampleLine(entity: Record<string, unknown>): { id: string; type: string; values: string } {
  const values = Object.entries(entity)
    .filter(([key]) => key !== "id" && key !== "type" && key !== "@context")
    .slice(0, 3)
    .map(([key, value]) => `${key}=${text(value)}`)
    .join(", ");
  return { id: String(entity.id ?? ""), type: String(entity.type ?? ""), values };
}

/** A value from the gateway, short enough for a table cell. Objects arrive when `keyValues` is ignored. */
function text(value: unknown): string {
  const rendered = typeof value === "object" && value !== null ? JSON.stringify(value) : String(value);
  return rendered.length > 40 ? `${rendered.slice(0, 39)}…` : rendered;
}

/**
 * What the chosen Endpoint actually gives you, before you describe the app (AP-51, UI-35).
 *
 * The point is that nobody has to read a manifest to decide: the grant is shown in words, the
 * data in five real entities, and the model behind both is one link away. Every request here
 * goes through the endpoint with the person's own session, so what they see is what they may
 * see, and an endpoint that answers nothing says so instead of blocking the form.
 */
export function EndpointPreview({ slug }: { slug: string }): JSX.Element {
  const { t } = useTranslation();
  const access = useAccess(slug);
  const samples = useQuery({
    queryKey: ["generator-endpoint-samples", slug],
    enabled: slug !== "",
    retry: false,
    // Nothing of one person's data outlives their page, as on the explore page (R17).
    gcTime: 0,
    queryFn: async () =>
      (await fetchJson(
        endpointUrl(slug, `/ngsi-ld/v1/entities?limit=${SAMPLE_LIMIT}&options=keyValues`),
      )) as Record<string, unknown>[],
  });

  const words = accessWords(access.data);
  const rows = (Array.isArray(samples.data) ? samples.data : []).slice(0, SAMPLE_LIMIT);

  return (
    <section aria-labelledby="generator-preview" className="space-y-3 rounded border border-border p-4">
      <div>
        <h2 id="generator-preview" className="text-base font-semibold">
          {t("apps.generate.preview.title")}
        </h2>
        <p className="mt-1 text-sm text-muted">{t("apps.generate.preview.hint")}</p>
      </div>

      <div className="space-y-1 text-sm">
        <h3 className="font-medium">{t("apps.generate.preview.accessTitle")}</h3>
        {access.isPending && <p role="status">{t("apps.generate.preview.loading")}</p>}
        {access.isError && <p className="text-muted">{t("apps.generate.preview.accessUnavailable")}</p>}
        {access.isSuccess && (
          <>
            {words.types.length > 0 && (
              <p>{t("apps.generate.preview.accessTypes", { types: words.types.join(", ") })}</p>
            )}
            <p>
              {words.attrs === "*"
                ? t("apps.generate.preview.accessAll")
                : t("apps.generate.preview.accessAttrs", { attrs: words.attrs.join(", ") })}
            </p>
            {words.denied.length > 0 && (
              <p className="text-muted">
                {t("apps.generate.preview.denied", { attrs: words.denied.join(", ") })}
              </p>
            )}
            <p className="text-muted">
              {words.writes.length > 0
                ? t("apps.generate.preview.alsoWrite", { actions: words.writes.join(", ") })
                : t("apps.generate.preview.readOnly")}
            </p>
          </>
        )}
      </div>

      <div className="space-y-1 text-sm">
        <h3 className="font-medium">{t("apps.generate.preview.samplesTitle")}</h3>
        {samples.isPending && <p role="status">{t("apps.generate.preview.loading")}</p>}
        {samples.isError && <p className="text-muted">{t("apps.generate.preview.samplesUnavailable")}</p>}
        {samples.isSuccess && rows.length === 0 && (
          <p className="text-muted">{t("apps.generate.preview.samplesEmpty")}</p>
        )}
        {rows.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-left text-xs">
              <caption className="sr-only">{t("apps.generate.preview.samplesTitle")}</caption>
              <thead>
                <tr className="border-b border-border">
                  <th scope="col" className="py-1 font-medium">
                    {t("apps.generate.preview.columnId")}
                  </th>
                  <th scope="col" className="py-1 font-medium">
                    {t("apps.generate.preview.columnType")}
                  </th>
                  <th scope="col" className="py-1 font-medium">
                    {t("apps.generate.preview.columnValues")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((entity, index) => {
                  const line = sampleLine(entity);
                  return (
                    <tr key={line.id || index} className="border-b border-border/50">
                      <th scope="row" className="py-1 pr-3 font-mono font-normal break-all">
                        {line.id}
                      </th>
                      <td className="py-1 pr-3">{line.type}</td>
                      <td className="py-1 font-mono break-all">{line.values}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <div className="space-y-1 text-sm">
        <h3 className="font-medium">{t("apps.generate.preview.linksTitle")}</h3>
        <ul className="flex flex-wrap gap-1">
          {ENDPOINT_LINKS.map((link) => (
            <li key={link.key}>
              <EndpointLink href={endpointUrl(slug, link.path)} muted>
                {t(`endpoints.link.${link.key}`)}
              </EndpointLink>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
