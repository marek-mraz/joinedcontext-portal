import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { SCHEMA_FORMALISMS } from "../../schemas/kinds";
import type { SchemaFormalism } from "../../schemas/kinds";

/**
 * What an Endpoint publishes of its model, and which attributes it holds back (EP-46, EP-61).
 *
 * The attribute names are read from the endpoint's own schema surface rather than from the
 * DataModel manifest, because the surface is already projected to the policy: what it lists
 * is what this endpoint may publish at all, so ticking a box here can only narrow further
 * (R9, EP-07). An attribute the steward hides is struck from the published column and never
 * reaches the preview, which is the same rule the gateway applies to the served document.
 */

/** One class of the published JSON Schema and the attributes it still carries. */
interface PublishedType {
  name: string;
  attributes: string[];
}

interface SchemaIndex {
  models?: { name?: string; version?: number }[];
}

/** Reads `$defs` of the projected draft-07 document the endpoint serves. */
export function publishedTypes(document: unknown): PublishedType[] {
  const defs = (document as { $defs?: Record<string, unknown> } | null)?.$defs;
  if (!defs) {
    return [];
  }
  return Object.entries(defs)
    .map(([name, definition]) => ({
      name,
      attributes: Object.keys(
        (definition as { properties?: Record<string, unknown> })?.properties ?? {},
      ).sort(),
    }))
    .filter((type) => type.attributes.length > 0);
}

/** A `Request` rather than a bare URL, so the caller reads like every other call the app makes. */
async function get(url: string, accept: string): Promise<Response> {
  const answer = await fetch(new Request(url, { headers: { Accept: accept } }));
  if (!answer.ok) {
    throw new Error(String(answer.status));
  }
  return answer;
}

async function fetchJson(url: string): Promise<unknown> {
  return (await get(url, "application/json")).json();
}

async function fetchText(url: string): Promise<string> {
  return (await get(url, "*/*")).text();
}

export interface SchemaProjectionPanelProps {
  /** The endpoint's slug; the schema surface lives under it. */
  slug: string;
  /** Attribute names this endpoint hides, `spec.projection.hiddenAttributes`. */
  hidden: string[];
  onHiddenChange: (hidden: string[]) => void;
}

export function SchemaProjectionPanel({
  slug,
  hidden,
  onHiddenChange,
}: SchemaProjectionPanelProps): JSX.Element {
  const { t } = useTranslation();
  const [formalism, setFormalism] = useState<SchemaFormalism>("json-schema");
  const [typed, setTyped] = useState("");
  const base = `${window.location.origin}/api/endpoint/${slug}/schema`;

  const index = useQuery({
    queryKey: ["endpoint-schema-index", slug],
    retry: false,
    queryFn: () => fetchJson(`${base}/index.json`) as Promise<SchemaIndex>,
  });

  // The major of the first model is the version segment every artifact of it lives under.
  const version = index.data?.models?.[0]?.version ?? 1;
  const artifactUrl = `${base}/v${version}/${formalism}`;

  const schema = useQuery({
    queryKey: ["endpoint-schema-artifact", slug, version, formalism],
    enabled: index.isSuccess,
    retry: false,
    queryFn: () =>
      formalism === "json-schema"
        ? (fetchJson(artifactUrl) as Promise<unknown>)
        : fetchText(artifactUrl),
  });

  const types = useMemo(
    () => (formalism === "json-schema" ? publishedTypes(schema.data) : []),
    [formalism, schema.data],
  );

  const toggle = (attribute: string) => {
    onHiddenChange(
      hidden.includes(attribute)
        ? hidden.filter((name) => name !== attribute)
        : [...hidden, attribute],
    );
  };

  return (
    <section aria-labelledby="projection-heading" className="space-y-3">
      <h3 id="projection-heading" className="text-base font-semibold">
        {t("endpoints.projection.title")}
      </h3>
      <p className="text-sm text-surface-fg/70">{t("endpoints.projection.hint")}</p>

      <label className="block text-sm font-medium" htmlFor="projection-formalism">
        {t("endpoints.projection.formalism")}
      </label>
      <select
        id="projection-formalism"
        value={formalism}
        onChange={(event) => setFormalism(event.target.value as SchemaFormalism)}
        className="block w-full rounded border border-border bg-surface px-3 py-1.5 text-base"
      >
        {SCHEMA_FORMALISMS.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </select>
      <p className="font-mono text-xs break-all text-surface-fg/60">{artifactUrl}</p>

      {index.isPending || schema.isPending ? (
        <p role="status">{t("endpoints.projection.loading")}</p>
      ) : null}

      {index.isError ? (
        <p className="text-sm text-surface-fg/70">{t("endpoints.projection.unavailable")}</p>
      ) : null}

      {index.isSuccess && schema.isError ? (
        <p className="text-sm text-surface-fg/70">{t("endpoints.projection.notCompiled")}</p>
      ) : null}

      {formalism !== "json-schema" && schema.isSuccess ? (
        <pre className="max-h-64 overflow-auto rounded border border-border bg-surface-subtle p-3 text-xs">
          {String(schema.data)}
        </pre>
      ) : null}

      {types.map((type) => {
        // The preview is built by subtraction, so a hidden attribute cannot appear in it
        // even if the published document still carries it.
        const effective = type.attributes.filter((name) => !hidden.includes(name));
        return (
          <table key={type.name} className="w-full border-collapse text-left text-sm">
            <caption className="py-1 text-left font-medium">{type.name}</caption>
            <thead>
              <tr className="border-b border-border">
                <th scope="col" className="py-1 font-medium">
                  {t("endpoints.projection.attribute")}
                </th>
                <th scope="col" className="py-1 font-medium">
                  {t("endpoints.projection.hide")}
                </th>
              </tr>
            </thead>
            <tbody>
              {type.attributes.map((attribute) => (
                <tr key={attribute} className="border-b border-border/50">
                  <th scope="row" className="py-1 font-mono font-normal">
                    {attribute}
                  </th>
                  <td className="py-1">
                    <input
                      type="checkbox"
                      aria-label={`${t("endpoints.projection.hide")} ${attribute}`}
                      checked={hidden.includes(attribute)}
                      onChange={() => toggle(attribute)}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <td colSpan={2} className="py-1 text-sm">
                  <span className="font-medium">{t("endpoints.projection.effective")}: </span>
                  {effective.length > 0 ? (
                    <span className="font-mono">{effective.join(", ")}</span>
                  ) : (
                    <span className="text-surface-fg/70">{t("endpoints.projection.none")}</span>
                  )}
                </td>
              </tr>
            </tfoot>
          </table>
        );
      })}

      <div className="flex items-end gap-2">
        <div className="flex-1">
          <label className="mb-1 block text-sm font-medium" htmlFor="projection-typed">
            {t("endpoints.projection.addHidden")}
          </label>
          <input
            id="projection-typed"
            value={typed}
            onChange={(event) => setTyped(event.target.value)}
            className="block w-full rounded border border-border bg-surface px-3 py-1.5 text-base"
          />
        </div>
        <button
          type="button"
          disabled={typed.trim() === "" || hidden.includes(typed.trim())}
          onClick={() => {
            onHiddenChange([...hidden, typed.trim()]);
            setTyped("");
          }}
          className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-not-allowed disabled:opacity-50"
        >
          {t("endpoints.projection.add")}
        </button>
      </div>

      {hidden.length > 0 ? (
        <ul className="flex flex-wrap gap-1">
          {hidden.map((attribute) => (
            <li key={attribute}>
              <button
                type="button"
                onClick={() => toggle(attribute)}
                className="inline-flex items-center gap-1 rounded border border-border px-2 py-0.5 font-mono text-xs hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
              >
                {attribute}
                <span aria-hidden="true">×</span>
                <span className="sr-only">{t("endpoints.projection.hide")}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
