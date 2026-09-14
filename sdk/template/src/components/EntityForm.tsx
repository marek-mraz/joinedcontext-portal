import { useEffect, useMemo, useState } from "react";
import { columnKind, fieldOf, format, pointOf, useAccess, useSave, useSchema } from "@joinedcontext/sdk";
import type { Cell, Field, Row } from "@joinedcontext/sdk";
import { Problem } from "./states";

export function parseInput(field: Field, text: string): { value: Cell } | { error: string } {
  if (text.trim() === "") {
    return { value: null };
  }
  if (field.input === "number") {
    const num = Number(text);
    if (Number.isNaN(num)) {
      return { error: "must be a number" };
    }
    if (field.min !== undefined && num < field.min) {
      return { error: `must be at least ${field.min}` };
    }
    if (field.max !== undefined && num > field.max) {
      return { error: `must be at most ${field.max}` };
    }
    return { value: num };
  }
  if (field.input === "checkbox") {
    return { value: text === "true" };
  }
  if (field.input === "geo") {
    const parts = text.trim().split(/[,\s]+/);
    if (parts.length === 2) {
      const lat = Number(parts[0]);
      const lon = Number(parts[1]);
      if (!Number.isNaN(lat) && !Number.isNaN(lon) && lat >= -90 && lat <= 90 && lon >= -180 && lon <= 180) {
        return { value: { type: "Point", coordinates: [lon, lat] } };
      }
    }
    return { error: 'must be "lat, lon"' };
  }
  if (field.input === "select") {
    if (field.options && !field.options.includes(text)) {
      return { error: `must be one of ${field.options.join(", ")}` };
    }
    return { value: text };
  }
  if (field.input === "date") {
    return { value: text };
  }
  if (field.pattern) {
    try {
      const re = new RegExp("^(?:" + field.pattern + ")$");
      if (!re.test(text)) {
        return { error: "does not match the expected format" };
      }
    } catch {
      // ignore invalid regex pattern
    }
  }
  return { value: text };
}

function getInitialDraft(
  r: Row | null | undefined,
  names: string[],
  specs: Record<string, Field>,
): Record<string, string> {
  const d: Record<string, string> = {};
  for (const name of names) {
    if (!r || r[name] === null || r[name] === undefined) {
      d[name] = "";
      continue;
    }
    const spec = specs[name];
    if (spec?.input === "geo") {
      const pt = pointOf(r[name]);
      d[name] = pt ? `${pt[1]}, ${pt[0]}` : "";
    } else if (spec?.input === "checkbox") {
      d[name] = r[name] === true ? "true" : "false";
    } else {
      d[name] = format(r[name]);
    }
  }
  return d;
}

export function EntityForm({
  type,
  row,
  fields,
  rows,
  title,
  onSaved,
  onCancel,
}: {
  type: string;
  row?: Row | null;
  fields?: string[];
  rows?: Row[];
  title?: string;
  onSaved?: (id: string) => void;
  onCancel?: () => void;
}): React.JSX.Element {
  const { typeSchema } = useSchema(type);
  const save = useSave();
  const { can } = useAccess();

  const isEdit = Boolean(row);
  const op = isEdit
    ? can("updateAttrs", type).ok
      ? "updateAttrs"
      : can("updateEntity", type).ok
        ? "updateEntity"
        : "updateAttrs"
    : "createEntity";

  const formDecision = can(op, type);

  const fieldNames = useMemo(() => {
    if (fields && fields.length > 0) return fields;
    if (typeSchema?.properties) {
      return Object.keys(typeSchema.properties).filter((k) => k !== "id" && k !== "type");
    }
    if (row) {
      return Object.keys(row).filter((k) => k !== "id" && k !== "type" && k !== "@context");
    }
    return [];
  }, [fields, typeSchema, row]);

  const fieldSpecs = useMemo(() => {
    const contextRows = rows ?? (row ? [row] : []);
    const map: Record<string, Field> = {};
    for (const name of fieldNames) {
      map[name] = fieldOf(name, typeSchema ?? undefined, columnKind(contextRows, name));
    }
    return map;
  }, [fieldNames, typeSchema, rows, row]);

  const [draft, setDraft] = useState<Record<string, string>>(() =>
    getInitialDraft(row, fieldNames, fieldSpecs),
  );
  const [initialDraft, setInitialDraft] = useState<Record<string, string>>(() =>
    getInitialDraft(row, fieldNames, fieldSpecs),
  );
  const [localId, setLocalId] = useState("");
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});

  const rowId = row?.id ?? null;
  const { clear } = save;
  // Another entity, or a new one: start over. Only its identity counts, so a parent that
  // re-renders or a refusal arriving never wipes what the person typed.
  useEffect(() => {
    const init = getInitialDraft(row, fieldNames, fieldSpecs);
    setDraft(init);
    setInitialDraft(init);
    setLocalId("");
    setFieldErrors({});
    clear();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rowId, type]);

  // Fields that become known later (the schema loads after the first render) join the draft
  // without touching the fields already there.
  useEffect(() => {
    const init = getInitialDraft(row, fieldNames, fieldSpecs);
    const merge = (d: Record<string, string>) => (fieldNames.every((n) => n in d) ? d : { ...init, ...d });
    setDraft(merge);
    setInitialDraft(merge);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fieldNames]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const errors: Record<string, string> = {};

    for (const name of fieldNames) {
      const spec = fieldSpecs[name];
      const text = draft[name] ?? "";
      if (spec.required && text.trim() === "") {
        errors[name] = `${name} is required`;
        continue;
      }
      if (text.trim() !== "") {
        const parsed = parseInput(spec, text);
        if ("error" in parsed) {
          errors[name] = `${name} ${parsed.error}`;
        }
      }
    }

    setFieldErrors(errors);
    if (Object.keys(errors).length > 0) {
      return;
    }

    if (!row) {
      const attrs: Record<string, Cell> = {};
      for (const name of fieldNames) {
        const spec = fieldSpecs[name];
        const text = draft[name] ?? "";
        if (text.trim() !== "") {
          const parsed = parseInput(spec, text);
          if ("value" in parsed && parsed.value !== null) {
            attrs[name] = parsed.value;
          }
        }
      }
      const newId = await save.create(type, attrs, localId.trim() || undefined);
      if (newId) {
        onSaved?.(newId);
      }
    } else {
      const patch: Record<string, Cell> = {};
      let changed = false;
      for (const name of fieldNames) {
        const spec = fieldSpecs[name];
        const text = draft[name] ?? "";
        const prev = initialDraft[name] ?? "";
        if (text !== prev) {
          changed = true;
          const parsed = parseInput(spec, text);
          if ("value" in parsed) {
            patch[name] = parsed.value;
          }
        }
      }
      if (!changed) {
        onSaved?.(row.id);
        return;
      }
      const ok = await save.update(row.id, patch);
      if (ok) {
        onSaved?.(row.id);
      }
    }
  };

  const submitDisabled = !formDecision.ok || save.saving;
  const submitTitle = !formDecision.ok ? formDecision.reason : undefined;

  return (
    <form
      className="jc-form"
      noValidate
      aria-label={title ?? (row ? "Edit " + type : "New " + type)}
      onSubmit={(e) => void handleSubmit(e)}
    >
      {!row && (
        <label className="jc-field">
          <span>Local id</span>
          <input
            name="localId"
            aria-label="Local id"
            value={localId}
            onChange={(e) => setLocalId(e.target.value)}
          />
        </label>
      )}
      {fieldNames.map((name) => {
        const spec = fieldSpecs[name];
        const val = draft[name] ?? "";
        const fieldDecision = can(op, type, name);
        const disabled = !fieldDecision.ok;
        const reason = disabled ? fieldDecision.reason : undefined;

        let inputElement: React.JSX.Element;
        switch (spec.input) {
          case "number":
            inputElement = (
              <input
                type="number"
                aria-label={name}
                min={spec.min}
                max={spec.max}
                step="any"
                value={val}
                disabled={disabled}
                title={reason}
                onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
              />
            );
            break;
          case "select":
            inputElement = (
              <select
                aria-label={name}
                value={val}
                disabled={disabled}
                title={reason}
                onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
              >
                <option value="">—</option>
                {spec.options?.map((opt) => (
                  <option key={opt} value={opt}>
                    {opt}
                  </option>
                ))}
              </select>
            );
            break;
          case "date":
            inputElement = (
              <input
                type="date"
                aria-label={name}
                value={val}
                disabled={disabled}
                title={reason}
                onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
              />
            );
            break;
          case "checkbox":
            inputElement = (
              <input
                type="checkbox"
                aria-label={name}
                checked={val === "true"}
                disabled={disabled}
                title={reason}
                onChange={(e) =>
                  setDraft((d) => ({ ...d, [name]: e.target.checked ? "true" : "false" }))
                }
              />
            );
            break;
          case "geo":
            inputElement = (
              <input
                type="text"
                aria-label={name}
                placeholder="lat, lon"
                value={val}
                disabled={disabled}
                title={reason}
                onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
              />
            );
            break;
          default:
            inputElement = (
              <input
                type="text"
                aria-label={name}
                value={val}
                disabled={disabled}
                title={reason}
                onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
              />
            );
            break;
        }

        return (
          <label key={name} className="jc-field">
            <span>
              {name}
              {spec.required ? " *" : ""}
            </span>
            {inputElement}
            {fieldErrors[name] && (
              <p className="jc-field-error" role="alert">
                {fieldErrors[name]}
              </p>
            )}
          </label>
        );
      })}
      {save.problem && <Problem error={save.problem} />}
      <div className="jc-form-actions">
        {onCancel && (
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        )}
        <button type="submit" disabled={submitDisabled} title={submitTitle}>
          {save.saving ? "Saving…" : "Save"}
        </button>
      </div>
    </form>
  );
}
