import { useEffect, useRef, useState } from "react";
import type { Cell, Row } from "../ngsi";
import { columnKind, format } from "../ngsi";

/**
 * The selected entity as a window of inputs. A save hands the changed cells back to the app,
 * which keeps them on screen: the map, the table and the tiles follow. Nothing is written to
 * the endpoint from here; the preview frame has no session, and a write route is a later slice.
 */
export function Form({ row, rows, fields, title, onSave, onClose }: { row: Row | null; rows: Row[]; fields: string[]; title?: string; onSave: (id: string, patch: Record<string, Cell>) => void; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [draft, setDraft] = useState<Record<string, string>>({});
  useEffect(() => {
    setDraft(row ? Object.fromEntries(fields.map((f) => [f, format(row[f])])) : {});
    const element = dialog.current;
    if (!element) return;
    if (row && !element.open) element.showModal();
    if (!row && element.open) element.close();
  }, [row, fields]);
  const kinds = Object.fromEntries(fields.map((f) => [f, columnKind(rows, f)]));
  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!row) return;
    const patch: Record<string, Cell> = {};
    for (const field of fields) {
      if (kinds[field] === "geo") continue;
      const text = draft[field] ?? "";
      const before = format(row[field]);
      if (text === before) continue;
      patch[field] = kinds[field] === "number" ? (text.trim() === "" ? null : Number(text)) : text;
    }
    onSave(row.id, patch);
    onClose();
  };
  return (
    <>
      <p className="empty">Pick a point on the map or a row in the table to open the form.</p>
      <dialog ref={dialog} className="form-window" onClose={onClose} aria-label={title ?? "Form"}>
        {row && (
          <form onSubmit={submit}>
            <h2>{title ?? "Entity"}</h2>
            <p className="mono">{row.id}</p>
            {fields.map((field) => (
              <label key={field} className="field">
                <span>{field}</span>
                {kinds[field] === "geo" ? (
                  <input value={draft[field] ?? ""} readOnly />
                ) : (
                  <input type={kinds[field] === "number" ? "number" : "text"} step="any" value={draft[field] ?? ""} onChange={(e) => setDraft((d) => ({ ...d, [field]: e.target.value }))} />
                )}
              </label>
            ))}
            <div className="form-actions">
              <button type="button" onClick={onClose}>Close</button>
              <button type="submit" className="primary">Save</button>
            </div>
            <p className="form-note">Saved on this screen; the endpoint is not written.</p>
          </form>
        )}
      </dialog>
    </>
  );
}
