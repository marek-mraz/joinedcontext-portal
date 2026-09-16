import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import {
  Alert,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
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
        <Alert role="status" tone="warning">
          {t("models.previewUnavailable")}
        </Alert>
      ) : null}
      {artifacts?.errors && artifacts.errors.length > 0 ? (
        <Alert role="status" tone="warning">
          <ul className="flex flex-col gap-1">
            {artifacts.errors.map((message) => (
              <li key={message}>{message}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {missing.length > 0 ? (
        <Alert role="alert" tone="danger">
          {t("models.exampleUnmapped", { terms: missing.join(", ") })}
        </Alert>
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
            <p className="text-body text-fg-muted">{t("models.noArtifact")}</p>
          )
        ) : null}
        {tab === "options" ? (
          <Table caption={t("models.tab.options")}>
            <TableHead>
              <TableHeaderCell>{t("models.slot")}</TableHeaderCell>
              <TableHeaderCell>{t("models.affordanceLabel")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {model.slots.map((slot) => (
                <TableRow key={slot.name}>
                  <TableCell primary>{slot.name}</TableCell>
                  <TableCell>{t(`models.affordance.${slotAffordance(slot)}`)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : null}
        {tab === "docs" ? (
          artifacts?.docs ? (
            <pre className="max-h-96 overflow-auto rounded-lg border border-border bg-surface-subtle p-3 font-mono text-xs text-fg">
              {artifacts.docs}
            </pre>
          ) : (
            <p className="text-body text-fg-muted">{t("models.noArtifact")}</p>
          )
        ) : null}
      </div>

      {artifacts?.generatorVersion ? (
        <p className="text-caption text-fg-muted">
          {t("models.generatedBy", { version: artifacts.generatorVersion })}
        </p>
      ) : null}
    </div>
  );
}
