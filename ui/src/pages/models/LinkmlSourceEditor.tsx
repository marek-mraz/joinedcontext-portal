import { Suspense, lazy, useCallback, useEffect, useRef } from "react";
import type { JSX } from "react";
import type { OnMount } from "@monaco-editor/react";
import type { editor, languages, Position } from "monaco-editor";
import { useTranslation } from "react-i18next";
import { NGSI_LD_KINDS, RANGES, UNIT_CODES } from "./linkml";
import type { Diagnostic } from "./linkml";

// Monaco is loaded when the source view is opened and not before: it is the heaviest thing in
// the Portal and every other view would otherwise pay for it.
const MonacoSourceView = lazy(() => import("./MonacoSourceView"));

/**
 * The YAML view of the same document the structured view edits (DM-13, DM-14).
 *
 * Both views hold one string: this one writes it directly, the structured view writes it
 * through the YAML document, and whichever is on screen shows what the other did. Validation
 * is the metamodel check of `linkml.ts` rendered as editor markers, so the source view and the
 * structured view can never disagree about what is wrong with the model.
 */
export interface LinkmlSourceEditorProps {
  source: string;
  onChange: (source: string) => void;
  diagnostics?: Diagnostic[];
  height?: string;
}

/**
 * Monaco's marker severities, as the numbers the editor uses.
 *
 * They are declared here rather than imported so the marker mapping stays a pure function that
 * a test can call without loading an editor.
 */
export const MARKER_SEVERITY = { error: 8, warning: 4 } as const;

export interface Marker {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
  message: string;
  severity: number;
}

/** The diagnostics of the model, as the markers Monaco underlines them with. */
export function markersFrom(diagnostics: Diagnostic[]): Marker[] {
  return diagnostics.map((diagnostic) => ({
    startLineNumber: diagnostic.line,
    startColumn: diagnostic.column,
    endLineNumber: diagnostic.line,
    // The end of the line: a metamodel problem is about the entry, not about one character.
    endColumn: diagnostic.column + 200,
    message: diagnostic.message,
    severity: MARKER_SEVERITY[diagnostic.severity],
  }));
}

/** The keys of the LinkML metamodel the editor completes, with what each one takes. */
export const METAMODEL_KEYS = [
  { label: "id", detail: "the model's own IRI" },
  { label: "name", detail: "the model's name" },
  { label: "title", detail: "a language map of titles" },
  { label: "prefixes", detail: "prefix → namespace" },
  { label: "default_prefix", detail: "the prefix new terms are minted under" },
  { label: "imports", detail: "linkml:types, ngsi-ld-core" },
  { label: "classes", detail: "the entity types" },
  { label: "slots", detail: "the attributes" },
  { label: "enums", detail: "the value sets" },
  { label: "class_uri", detail: "the IRI of a class" },
  { label: "slot_uri", detail: "the IRI of a slot" },
  { label: "range", detail: "the type of a slot's value" },
  { label: "required", detail: "true when a payload must carry the slot" },
  { label: "multivalued", detail: "true when the slot carries a list" },
  { label: "deprecated", detail: "true keeps the slot valid and warns consumers" },
  { label: "unit", detail: "ucum_code, with the UN/CEFACT code in exact_mappings" },
  { label: "annotations", detail: "ngsi_ld_kind and upstream_source live here" },
  { label: "permissible_values", detail: "the values of an enum" },
  { label: "description", detail: "what the term means" },
] as const;

export interface Completion {
  label: string;
  insertText: string;
  detail?: string;
}

/**
 * What may be written at this point of the document (DM-14).
 *
 * The suggestion set follows the key the line is under, because the LinkML metamodel is
 * position-dependent: a range only makes sense inside a slot, a kind only inside annotations.
 */
export function completionsFor(line: string): Completion[] {
  const trimmed = line.trimStart();
  if (/(^|\s)ngsi_ld_kind:\s*\S*$/.test(trimmed)) {
    return NGSI_LD_KINDS.map((kind) => ({ label: kind, insertText: kind }));
  }
  if (/(^|\s)range:\s*\S*$/.test(trimmed)) {
    return RANGES.map((range) => ({ label: range, insertText: range }));
  }
  if (/(^|\s)ucum_code:\s*\S*$/.test(trimmed)) {
    return UNIT_CODES.map((unit) => ({
      label: unit.ucum,
      insertText: unit.ucum,
      detail: `${unit.code} · ${unit.label}`,
    }));
  }
  return METAMODEL_KEYS.map((key) => ({
    label: key.label,
    insertText: `${key.label}: `,
    detail: key.detail,
  }));
}

export function LinkmlSourceEditor({
  source,
  onChange,
  diagnostics = [],
  height = "24rem",
}: LinkmlSourceEditorProps): JSX.Element {
  const { t } = useTranslation();
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);
  const monacoRef = useRef<Parameters<OnMount>[1] | null>(null);

  const paint = useCallback((markers: Marker[]) => {
    const editor = editorRef.current;
    const monaco = monacoRef.current;
    const model = editor?.getModel();
    if (!editor || !monaco || !model) {
      return;
    }
    monaco.editor.setModelMarkers(model, "linkml", markers);
  }, []);

  useEffect(() => {
    paint(markersFrom(diagnostics));
  }, [diagnostics, paint]);

  const onMount: OnMount = (editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;
    const provider: languages.CompletionItemProvider = {
      provideCompletionItems: (model: editor.ITextModel, position: Position) => {
        const line = model.getValueInRange({
          startLineNumber: position.lineNumber,
          startColumn: 1,
          endLineNumber: position.lineNumber,
          endColumn: position.column,
        });
        const word = model.getWordUntilPosition(position);
        const range = {
          startLineNumber: position.lineNumber,
          endLineNumber: position.lineNumber,
          startColumn: word.startColumn,
          endColumn: word.endColumn,
        };
        return {
          suggestions: completionsFor(line).map((completion) => ({
            label: completion.label,
            kind: monaco.languages.CompletionItemKind.Property,
            insertText: completion.insertText,
            detail: completion.detail,
            range,
          })),
        };
      },
    };
    monaco.languages.registerCompletionItemProvider("yaml", provider);
    paint(markersFrom(diagnostics));
  };

  const errors = diagnostics.filter((diagnostic) => diagnostic.severity === "error");
  const warnings = diagnostics.filter((diagnostic) => diagnostic.severity === "warning");

  return (
    <div className="flex flex-col gap-3">
      <div className="overflow-hidden rounded border border-border">
        <Suspense fallback={<p className="p-3 text-sm">{t("models.loadingEditor")}</p>}>
          <MonacoSourceView
            value={source}
            onChange={onChange}
            onMount={onMount}
            height={height}
          />
        </Suspense>
      </div>
      <section aria-labelledby="models-diagnostics">
        <h3 id="models-diagnostics" className="text-sm font-semibold">
          {t("models.diagnostics", { errors: errors.length, warnings: warnings.length })}
        </h3>
        {diagnostics.length === 0 ? (
          <p className="text-sm text-surface-fg/70">{t("models.noDiagnostics")}</p>
        ) : (
          <ul className="mt-1 flex flex-col gap-1 text-sm">
            {diagnostics.map((diagnostic, index) => (
              <li
                key={`${diagnostic.line}-${index}`}
                className={
                  diagnostic.severity === "error" ? "text-danger-fg" : "text-warning-fg"
                }
              >
                <span className="font-mono text-xs">
                  {t("models.atLine", { line: diagnostic.line, column: diagnostic.column })}
                </span>{" "}
                {diagnostic.message}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
