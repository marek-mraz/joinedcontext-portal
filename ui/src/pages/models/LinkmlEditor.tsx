import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { LinkmlPreviewPanel } from "./LinkmlPreviewPanel";
import { LinkmlGraphView } from "./LinkmlGraphView";
import { LinkmlSourceEditor } from "./LinkmlSourceEditor";
import { LinkmlVisualEditor } from "./LinkmlVisualEditor";
import { ModelSubsetPicker } from "./ModelSubsetPicker";
import { diagnose, parseModel } from "./linkml";
import { EMPTY_SUBSET, subsetSource } from "./subset";
import type { Subset } from "./subset";

/**
 * The one LinkML editor every page embeds (DM-13, DM-17).
 *
 * It is controlled: the document is the `source` string the caller holds, every view edits
 * that string through `onChange`, and nothing here has a second copy. That is also what makes
 * it drivable from outside — an assistant applies `applyOperations` to the same string and the
 * tree shows the result (DM-31), a test sets the string and reads the tree.
 *
 * Two modes, one component. With `onSubsetChange` the editor is a picker: the model as a tree of
 * checkboxes and the preview of the narrowed model an endpoint would serve (MP-01, MP-03).
 * Without it, the editor edits: structure, source and preview of the whole model.
 */
export type EditorView = "structure" | "source" | "graph" | "preview" | "subset";

export interface LinkmlEditorProps {
  source: string;
  onChange: (source: string) => void;
  /** The organisation's locales, for the language maps every title needs (DM-15). */
  locales?: string[];
  /** Subset mode: the classes and slots ticked so far, and where a tick goes. */
  subset?: Subset;
  onSubsetChange?: (subset: Subset) => void;
  initialView?: EditorView;
}

const EDIT_VIEWS: EditorView[] = ["structure", "source", "graph", "preview"];
const SUBSET_VIEWS: EditorView[] = ["subset", "preview"];

export function LinkmlEditor({
  source,
  onChange,
  locales = [],
  subset,
  onSubsetChange,
  initialView,
}: LinkmlEditorProps): JSX.Element {
  const { t } = useTranslation();
  const picking = onSubsetChange !== undefined;
  const views = picking ? SUBSET_VIEWS : EDIT_VIEWS;
  const [view, setView] = useState<EditorView>(initialView ?? views[0]);

  const model = useMemo(() => parseModel(source), [source]);
  const diagnostics = useMemo(() => diagnose(source, locales), [source, locales]);
  const chosen = subset ?? EMPTY_SUBSET;
  const previewed = useMemo(
    () => (picking ? subsetSource(source, chosen) : source),
    [picking, source, chosen],
  );

  return (
    <div className="flex flex-col gap-3">
      <div role="tablist" aria-label={t("models.title")} className="flex flex-wrap gap-1">
        {views.map((name) => (
          <button
            key={name}
            type="button"
            role="tab"
            aria-selected={view === name}
            onClick={() => setView(name)}
            className={
              view === name
                ? "rounded-md border border-border bg-surface-subtle px-3 py-1 text-body font-medium text-fg"
                : "rounded-md border border-transparent px-3 py-1 text-body text-fg-muted hover:bg-surface-subtle hover:text-fg"
            }
          >
            {t(`models.view.${name}`)}
          </button>
        ))}
      </div>
      <div role="tabpanel">
        {view === "structure" ? (
          <LinkmlVisualEditor
            source={source}
            onChange={onChange}
            diagnostics={diagnostics}
            locales={locales}
          />
        ) : null}
        {view === "graph" ? (
          // Read-only, and clicking a class opens it where it can be edited (T-1111).
          <LinkmlGraphView source={source} onOpenClass={() => setView("structure")} />
        ) : null}
        {view === "source" ? (
          <LinkmlSourceEditor source={source} onChange={onChange} diagnostics={diagnostics} />
        ) : null}
        {view === "subset" && onSubsetChange ? (
          <ModelSubsetPicker model={model} subset={chosen} onChange={onSubsetChange} />
        ) : null}
        {view === "preview" ? <LinkmlPreviewPanel source={previewed} /> : null}
      </div>
    </div>
  );
}
