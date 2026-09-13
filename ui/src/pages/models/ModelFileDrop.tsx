import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { readCsrfToken } from "../../api/client";
import { Badge } from "../../components/ui/Badge";
import { Button } from "../../components/ui/Button";
import { Dialog } from "../../components/ui/Dialog";
import { blankSource } from "./linkml";
import { applyOperations } from "./operations";
import type { Operation, Refusal } from "./operations";

/**
 * A model from a dropped file (T-0599, DM-54, DM-55): the sample goes to the Portal, Model
 * Tools reads it once in memory and answers a draft as editor operations; the preview shows
 * what was inferred, and "Populate the editor" applies the operations to a blank model through
 * the same `applyOperations` a person's clicks go through (DM-13). Every string of the answer
 * is text from the file and is rendered as text, never as markup.
 */

export const MAX_SAMPLE_BYTES = 10 * 1024 * 1024;
export const ACCEPT = ".csv,.tsv,.xlsx,.json,.pdf,text/csv,application/json,application/pdf";

export interface InferAnswer {
  linkml: string;
  operations: Operation[];
  detectedTypes: Record<string, string>;
  matches: Record<string, { model: string; slotUri: string }>;
  untyped: { slot: string; reason: string }[];
  rows: number;
}

/** The answer of `POST /api/v1/tools/infer-schema`, or none when it is not what Model Tools writes. */
export function inferAnswerOf(value: unknown): InferAnswer | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const answer = value as Record<string, unknown>;
  if (typeof answer.linkml !== "string" || !Array.isArray(answer.operations)) {
    return null;
  }
  const operations = answer.operations.filter(
    (op): op is Operation => typeof op === "object" && op !== null && typeof (op as { op?: unknown }).op === "string",
  );
  const matches: InferAnswer["matches"] = {};
  for (const [slot, match] of Object.entries(record(answer.matches))) {
    const entry = record(match);
    if (typeof entry.model === "string" && typeof entry.slotUri === "string") {
      matches[slot] = { model: entry.model, slotUri: entry.slotUri };
    }
  }
  const untyped = (Array.isArray(answer.untyped) ? answer.untyped : [])
    .map(record)
    .filter((entry) => typeof entry.slot === "string")
    .map((entry) => ({ slot: String(entry.slot), reason: typeof entry.reason === "string" ? entry.reason : "" }));
  const detectedTypes: Record<string, string> = {};
  for (const [slot, type] of Object.entries(record(answer.detectedTypes))) {
    if (typeof type === "string") {
      detectedTypes[slot] = type;
    }
  }
  return {
    linkml: answer.linkml,
    operations,
    detectedTypes,
    matches,
    untyped,
    rows: typeof answer.rows === "number" ? answer.rows : 0,
  };
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

export interface Draft {
  name: string;
  answer: InferAnswer;
  /** The blank model with the operations applied: what the editor opens with. */
  source: string;
  refused: Refusal[];
}

/** The draft as the editor will hold it: the operations applied to a blank model of the organisation. */
export function draftOf(answer: InferAnswer, domain: string, fileName: string): Draft {
  const stem =
    fileName
      .replace(/\.[^.]+$/, "")
      .replace(/[^A-Za-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .toLowerCase() || "sample";
  const applied = applyOperations(blankSource(domain, stem), answer.operations);
  return { name: fileName, answer, source: applied.source, refused: applied.refused };
}

interface SlotRow {
  klass: string;
  name: string;
  title?: string;
  range?: string;
  kind?: string;
  unit?: string;
}

/** The slots of the draft, one row each, as the operations declare them. */
export function slotRows(operations: Operation[]): SlotRow[] {
  const rows: SlotRow[] = [];
  let klass = "";
  for (const op of operations) {
    if (op.op === "addClass") {
      klass = op.name;
    } else if (op.op === "addSlot") {
      rows.push({ klass: op.class ?? klass, name: op.name, range: op.range, kind: op.kind });
    } else if (op.op === "setTitle" && op.target === "slot") {
      const row = rows.find((r) => r.name === op.name);
      if (row) {
        row.title = op.value;
      }
    } else if (op.op === "setSlot" && op.field === "unit") {
      const row = rows.find((r) => r.name === op.name);
      if (row) {
        row.unit = String(op.value);
      }
    }
  }
  return rows;
}

export function ModelFileDrop({
  project,
  onPopulate,
  compact = false,
}: {
  project: string;
  /** The draft source the editor opens with, and the answer it came from. */
  onPopulate: (source: string, answer: InferAnswer) => void;
  /** The label and input only, for a dock with no room for the lead. */
  compact?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);

  const takeFile = async (file: File | undefined): Promise<void> => {
    if (!file) {
      return;
    }
    setProblem(null);
    if (file.size > MAX_SAMPLE_BYTES) {
      setProblem(t("models.infer.tooLarge"));
      return;
    }
    const form = new FormData();
    form.append("file", file, file.name);
    const csrf = readCsrfToken();
    setBusy(file.name);
    try {
      const response = await fetch("/api/v1/tools/infer-schema", {
        method: "POST",
        body: form,
        credentials: "same-origin",
        headers: csrf ? { "x-csrf-token": csrf } : {},
      });
      if (!response.ok) {
        const detail = record(await response.json().catch(() => null)).detail;
        setProblem(t("models.infer.failed", { detail: typeof detail === "string" ? detail : String(response.status) }));
        return;
      }
      const answer = inferAnswerOf(await response.json());
      if (!answer) {
        setProblem(t("models.infer.unreadable"));
        return;
      }
      setDraft(draftOf(answer, `${project}.sk`, file.name));
    } catch {
      setProblem(t("models.infer.failed", { detail: "network" }));
    } finally {
      setBusy(null);
    }
  };

  const rows = draft ? slotRows(draft.answer.operations) : [];
  const classes = draft ? draft.answer.operations.filter((op) => op.op === "addClass").length : 0;

  return (
    <section
      aria-label={t("models.infer.title")}
      className={
        compact
          ? "flex flex-col gap-1"
          : "flex flex-col gap-2 rounded-md border border-dashed border-border bg-surface-subtle p-3"
      }
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => {
        event.preventDefault();
        void takeFile(event.dataTransfer.files[0]);
      }}
    >
      {compact ? null : (
        <>
          <h3 className="text-body font-semibold text-fg">{t("models.infer.title")}</h3>
          <p className="text-caption text-fg-muted">{t("models.infer.lead")}</p>
        </>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <label className="cursor-pointer text-body">
          <span className="rounded-md border border-border bg-surface px-3 py-2">{t("models.infer.drop")}</span>
          <input
            type="file"
            accept={ACCEPT}
            aria-label={t("models.infer.chooseFile")}
            className="sr-only"
            onChange={(event) => {
              void takeFile(event.target.files?.[0]);
              event.target.value = "";
            }}
          />
        </label>
        {busy ? <span className="text-caption text-fg-muted">{t("models.infer.reading", { name: busy })}</span> : null}
      </div>
      {problem ? (
        <p role="alert" className="text-sm text-danger-fg">
          {problem}
        </p>
      ) : null}
      <Dialog
        open={draft !== null}
        onOpenChange={(open) => {
          if (!open) {
            setDraft(null);
          }
        }}
        title={t("models.infer.preview", { name: draft?.name ?? "" })}
        description={t("models.infer.previewLead", { rows: draft?.answer.rows ?? 0, classes })}
        size="lg"
        closeLabel={t("models.infer.close")}
        footer={
          <>
            <Button
              onClick={() => {
                setDraft(null);
              }}
            >
              {t("models.infer.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={draft === null || draft.refused.length > 0}
              onClick={() => {
                if (draft) {
                  onPopulate(draft.source, draft.answer);
                  setDraft(null);
                }
              }}
            >
              {t("models.infer.populate")}
            </Button>
          </>
        }
      >
        {draft ? (
          <div className="flex flex-col gap-3 text-sm">
            <div className="overflow-x-auto">
              <table className="w-full text-left text-xs">
                <thead>
                  <tr className="text-fg-muted">
                    <th className="pr-3">{t("models.infer.class")}</th>
                    <th className="pr-3">{t("models.infer.column")}</th>
                    <th className="pr-3">{t("models.infer.slot")}</th>
                    <th className="pr-3">{t("models.infer.range")}</th>
                    <th className="pr-3">{t("models.infer.kind")}</th>
                    <th className="pr-3">{t("models.infer.unit")}</th>
                    <th>{t("models.infer.match")}</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => (
                    <tr key={`${row.klass}/${row.name}`} className="border-t border-border">
                      <td className="pr-3 font-mono">{row.klass}</td>
                      <td className="max-w-[16rem] break-words pr-3">{row.title ?? row.name}</td>
                      <td className="pr-3 font-mono">{row.name}</td>
                      <td className="pr-3">
                        <Badge mono>{row.range ?? "string"}</Badge>
                      </td>
                      <td className="pr-3">{row.kind ?? "Property"}</td>
                      <td className="pr-3 font-mono">{row.unit ?? ""}</td>
                      <td className="break-all font-mono">{draft.answer.matches[row.name]?.model ?? ""}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {draft.answer.untyped.length > 0 ? (
              <div>
                <p className="font-medium">{t("models.infer.untyped")}</p>
                <ul className="list-disc pl-5 text-xs">
                  {draft.answer.untyped.map((entry) => (
                    <li key={entry.slot}>
                      <span className="font-mono">{entry.slot}</span>: {entry.reason}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
            {draft.refused.length > 0 ? (
              <div role="alert" className="text-xs text-danger-fg">
                <p>{t("models.infer.refused", { n: draft.refused.length })}</p>
                <ul className="list-disc pl-5">
                  {draft.refused.map((refusal) => (
                    <li key={refusal.index}>{refusal.reason}</li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ) : null}
      </Dialog>
    </section>
  );
}
