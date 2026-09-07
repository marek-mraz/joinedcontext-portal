import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { parseModel, slotAffordance } from "./linkml";

/**
 * What the model compiles to, refreshed while the user types (DM-17).
 *
 * Every artifact comes from Model Tools through the Portal: the browser never compiles a
 * schema itself and never reaches a third party (DM-10, DM-18). A source that does not compile
 * yet answers with messages and no artifacts, which is the normal state of an editor, so the
 * panel keeps the last good preview visible next to the messages instead of blanking.
 */
export interface LinkmlPreviewPanelProps {
  source: string;
  /** How long to wait after the last keystroke before compiling. */
  debounceMs?: number;
}

export interface Artifacts {
  /** The LinkML source itself; an import answers with it, a compilation does not (DM-07). */
  linkml?: string;
  jsonSchema?: Record<string, unknown>;
  context?: Record<string, unknown>;
  /** The documentation page Model Tools renders, which is the one the repository commits. */
  docs?: string;
  shacl?: string;
  owl?: string;
  example?: Record<string, unknown>;
  generatorVersion?: string;
  errors?: string[];
}

type Tab = "schema" | "context" | "example" | "form" | "options" | "docs";

const TABS: Tab[] = ["schema", "context", "example", "form", "options", "docs"];

/** The JSON-LD keywords an example carries that no `@context` has to define. */
const KEYWORDS = ["id", "type", "@context", "@id", "@type"];

/**
 * The attributes of the example that the generated `@context` does not define (DM-21).
 *
 * An example that expands with warnings is an example that would federate as opaque data, so
 * the panel says which term is missing rather than showing a green preview of a broken model.
 */
export function unmappedTerms(
  example: Record<string, unknown> | undefined,
  context: Record<string, unknown> | undefined,
): string[] {
  if (!example) {
    return [];
  }
  const defined = new Set(
    Object.keys(
      (context?.["@context"] as Record<string, unknown> | undefined) ?? context ?? {},
    ),
  );
  return Object.keys(example).filter(
    (term) => !KEYWORDS.includes(term) && !defined.has(term) && !term.startsWith("@"),
  );
}

function Json({ value }: { value: unknown }): JSX.Element {
  return (
    <pre className="max-h-96 overflow-auto rounded border border-border bg-surface-subtle p-3 text-xs">
      {JSON.stringify(value, null, 2)}
    </pre>
  );
}

export function LinkmlPreviewPanel({
  source,
  debounceMs = 400,
}: LinkmlPreviewPanelProps): JSX.Element {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("schema");
  const [settled, setSettled] = useState(source);

  // The editor asks on every pause in typing, not on every keystroke: Model Tools is shared
  // and a compile per character would queue behind itself.
  useEffect(() => {
    const timer = setTimeout(() => setSettled(source), debounceMs);
    return () => clearTimeout(timer);
  }, [source, debounceMs]);

  const preview = useQuery({
    queryKey: ["tools", "generate", settled],
    enabled: settled.trim().length > 0,
    // A preview is a function of the source, so a source seen before never needs recompiling.
    staleTime: Infinity,
    retry: false,
    queryFn: async (): Promise<Artifacts> =>
      unwrap(
        await api.POST("/api/v1/tools/generate", {
          body: { source: settled },
        }),
      ) as Artifacts,
  });

  const model = useMemo(() => parseModel(settled), [settled]);
  const artifacts = preview.data;
  const missing = useMemo(
    () => unmappedTerms(artifacts?.example, artifacts?.context),
    [artifacts],
  );

  return (
    <div className="flex flex-col gap-3">
      <div role="tablist" aria-label={t("models.preview")} className="flex flex-wrap gap-1">
        {TABS.map((name) => (
          <button
            key={name}
            type="button"
            role="tab"
            aria-selected={tab === name}
            onClick={() => setTab(name)}
            className={
              tab === name
                ? "rounded border border-border bg-surface-subtle px-2 py-1 text-sm font-medium"
                : "rounded border border-transparent px-2 py-1 text-sm hover:bg-surface-subtle"
            }
          >
            {t(`models.tab.${name}`)}
          </button>
        ))}
      </div>

      {preview.isError ? (
        <p role="status" className="text-sm text-warning-fg">
          {t("models.previewUnavailable")}
        </p>
      ) : null}
      {artifacts?.errors && artifacts.errors.length > 0 ? (
        <ul role="status" className="flex flex-col gap-1 text-sm text-warning-fg">
          {artifacts.errors.map((message) => (
            <li key={message}>{message}</li>
          ))}
        </ul>
      ) : null}
      {missing.length > 0 ? (
        <p role="alert" className="text-sm text-danger-fg">
          {t("models.exampleUnmapped", { terms: missing.join(", ") })}
        </p>
      ) : null}

      <div role="tabpanel">
        {tab === "schema" ? <Json value={artifacts?.jsonSchema ?? null} /> : null}
        {tab === "context" ? <Json value={artifacts?.context ?? null} /> : null}
        {tab === "example" ? <Json value={artifacts?.example ?? null} /> : null}
        {tab === "form" ? (
          artifacts?.jsonSchema ? (
            <SchemaForm
              schema={artifacts.jsonSchema as JsonSchema}
              formData={artifacts.example}
              disabled
              onSubmit={() => undefined}
              submitLabel={t("models.formPreview")}
            />
          ) : (
            <p className="text-sm text-surface-fg/70">{t("models.noArtifact")}</p>
          )
        ) : null}
        {tab === "options" ? (
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="text-xs uppercase text-surface-fg/70">
                <th scope="col" className="py-1">{t("models.slot")}</th>
                <th scope="col">{t("models.affordanceLabel")}</th>
              </tr>
            </thead>
            <tbody>
              {model.slots.map((slot) => (
                <tr key={slot.name} className="border-t border-border">
                  <th scope="row" className="py-1 font-normal">
                    {slot.name}
                  </th>
                  <td>{t(`models.affordance.${slotAffordance(slot)}`)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
        {tab === "docs" ? (
          artifacts?.docs ? (
            <pre className="max-h-96 overflow-auto rounded border border-border bg-surface-subtle p-3 text-xs">
              {artifacts.docs}
            </pre>
          ) : (
            <p className="text-sm text-surface-fg/70">{t("models.noArtifact")}</p>
          )
        ) : null}
      </div>

      {artifacts?.generatorVersion ? (
        <p className="text-xs text-surface-fg/70">
          {t("models.generatedBy", { version: artifacts.generatorVersion })}
        </p>
      ) : null}
    </div>
  );
}
