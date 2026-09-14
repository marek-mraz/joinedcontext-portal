import { useEffect, useRef, useState } from "react";
import type { Cell, Row } from "../ngsi";
import { columnKind, format } from "../ngsi";
import type { Field, TypeSchema, WriteResult } from "../write";
import { fieldOf } from "../write";

/**
 * The selected entity as a window of inputs, or a new one (AP-61, AP-62). Each input is what
 * the endpoint's schema says the attribute is: a select over an enum, a number within its
 * bounds, a pattern, a required mark. A save writes through the endpoint; a refusal stays on
 * the form beside the inputs with the reason, and nothing reloads.
 */
export function Form({ row, rows, fields, title, schema, creating, onSave, onClose }: { row: Row | null; rows: Row[]; fields: string[]; title?: string; schema?: TypeSchema; creating: boolean; onSave: (id: string | null, patch: Record<string, Cell>) => Promise<WriteResult>; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [localId, setLocalId] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const open = creating || row !== null;
  useEffect(() => {
    setDraft(row ? Object.fromEntries(fields.map((f) => [f, format(row[f])])) : {});
    setLocalId("");
    setProblem(null);
    const element = dialog.current;
    if (!element) return;
    if (open && !element.open) element.showModal();
    if (!open && element.open) element.close();
  }, [row, fields, open]);
  const specs: Record<string, Field> = Object.fromEntries(fields.map((f) => [f, fieldOf(f, schema, columnKind(rows, f))]));
  const idPrefix = rows[0]?.id.includes(":") ? rows[0].id.slice(0, rows[0].id.lastIndexOf(":") + 1) : "";

  // The app runs in a frame sandboxed without `allow-forms` (AP-63), where the browser never
  // starts a form submission, so no submit event ever reaches React: the Save button and the
  // Enter key call this themselves, after the inputs' own validity check.
  const submit = async () => {
    if (!form.current?.reportValidity()) return;
    const patch: Record<string, Cell> = {};
    for (const field of fields) {
      const spec = specs[field];
      if (spec.input === "geo") continue;
      const text = draft[field] ?? "";
      if (row && text === format(row[field])) continue;
      if (spec.input === "number") patch[field] = text.trim() === "" ? null : Number(text);
      else if (spec.input === "checkbox") patch[field] = text === "true";
      else patch[field] = text;
    }
    if (!row && !creating) return;
    setSaving(true);
    setProblem(null);
    const result = await onSave(row ? row.id : `${idPrefix}${localId.trim()}`, patch);
    setSaving(false);
    if (result.ok) {
      onClose();
    } else {
      setProblem(result.detail ?? `The endpoint answered ${result.status}.`);
    }
  };

  const input = (field: string) => {
    const spec = specs[field];
    const value = draft[field] ?? "";
    const set = (next: string) => setDraft((d) => ({ ...d, [field]: next }));
    switch (spec.input) {
      case "geo":
        return <input value={value} readOnly />;
      case "select":
        return (
          <select value={value} required={spec.required} onChange={(e) => set(e.target.value)}>
            <option value="">—</option>
            {spec.options?.map((option) => (
              <option key={option} value={option}>{option}</option>
            ))}
          </select>
        );
      case "number":
        return <input type="number" step="any" min={spec.min} max={spec.max} required={spec.required} value={value} onChange={(e) => set(e.target.value)} />;
      case "checkbox":
        return <input type="checkbox" checked={value === "true"} onChange={(e) => set(e.target.checked ? "true" : "false")} />;
      case "date":
        return <input type="datetime-local" required={spec.required} value={value.replace(/Z$/, "").slice(0, 16)} onChange={(e) => set(e.target.value === "" ? "" : `${e.target.value}:00Z`)} />;
      default:
        return <input type="text" pattern={spec.pattern} required={spec.required} value={value} onChange={(e) => set(e.target.value)} />;
    }
  };

  return (
    <>
      <p className="empty">Pick a point on the map or a row in the table to open the form.</p>
      <dialog ref={dialog} className="form-window" onClose={onClose} aria-label={title ?? "Form"}>
        {open && (
          <form
            ref={form}
            onSubmit={(e) => e.preventDefault()}
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.target instanceof HTMLInputElement) {
                e.preventDefault();
                void submit();
              }
            }}
          >
            <h2>{title ?? (row ? "Entity" : "New entity")}</h2>
            {row ? (
              <p className="mono">{row.id}</p>
            ) : (
              <label className="field">
                <span>id</span>
                <input type="text" aria-label="id" required pattern="[A-Za-z0-9._~-]+" placeholder={`${idPrefix}…`} value={localId} onChange={(e) => setLocalId(e.target.value)} />
              </label>
            )}
            {fields.map((field) => (
              <label key={field} className="field">
                <span>{field}{specs[field].required ? " *" : ""}</span>
                {input(field)}
              </label>
            ))}
            {problem && <p role="alert" className="error">{problem}</p>}
            <div className="form-actions">
              <button type="button" onClick={onClose}>Close</button>
              <button type="button" className="primary" disabled={saving} onClick={() => void submit()}>{saving ? "Saving…" : "Save"}</button>
            </div>
            <p className="form-note">Written through the endpoint with your own access; the Policy decides.</p>
          </form>
        )}
      </dialog>
    </>
  );
}
