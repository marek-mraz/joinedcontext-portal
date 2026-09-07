import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Button, Field, Input, Select } from "../ui";
import { filtersFromQ, opsFor, qFromFilters } from "./filters";
import type { EntityQuery, Filter, FilterSlot } from "./filters";

export interface EntityFiltersProps {
  /** The prefix of the control ids: `${id}-type`, `${id}-q`, `${id}-scopeQ`. */
  id: string;
  /** The entity types the space's DataModel declares; a free text field when it declares none. */
  types: string[];
  /** The attributes of the chosen type, typed by the model (UI-33); may be empty. */
  slots: FilterSlot[];
  value: EntityQuery;
  onChange: (next: EntityQuery) => void;
}

/**
 * The filters of one entity query, generated from the model (UI-33): the type, one row per
 * filter with the operators and input the slot's range allows, `scopeQ`, and the attributes to
 * read. Every row is a view of `value.q`; a `q` the rows cannot show stays a text field.
 */
export function EntityFilters({ id, types, slots, value, onChange }: EntityFiltersProps): JSX.Element {
  const { t } = useTranslation();
  const q = value.q ?? "";
  // The rows are derived from `q`, but a row being typed (no value yet) is not in `q`, so they
  // live here and are re-derived only when `q` changed under them (the YAML view, a reset).
  const [state, setState] = useState(() => ({ q, rows: filtersFromQ(q) }));
  if (q !== state.q) {
    setState({ q, rows: filtersFromQ(q) });
  }
  const rows = q === state.q ? state.rows : filtersFromQ(q);

  function setRows(next: Filter[]) {
    const composed = qFromFilters(next, slots);
    setState({ q: composed, rows: next });
    onChange({ ...value, q: composed || undefined });
  }

  function setRow(index: number, patch: Partial<Filter>) {
    setRows(rows!.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function toggleAttribute(attribute: string, on: boolean) {
    const current = value.attrs ?? [];
    const attrs = on ? [...current, attribute] : current.filter((a) => a !== attribute);
    onChange({ ...value, attrs: attrs.length > 0 ? attrs : undefined });
  }

  return (
    <div className="flex flex-col gap-3" data-testid={`${id}-filters`}>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id={`${id}-type`} label={t("entities.type")}>
          {types.length > 0 ? (
            <Select
              id={`${id}-type`}
              value={value.type ?? ""}
              onChange={(event) =>
                onChange({ type: event.target.value || undefined, scopeQ: value.scopeQ })
              }
            >
              <option value="">—</option>
              {types.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          ) : (
            <Input
              id={`${id}-type`}
              value={value.type ?? ""}
              placeholder={t("entities.typePlaceholder")}
              onChange={(event) => onChange({ ...value, type: event.target.value || undefined })}
            />
          )}
        </Field>
        <Field id={`${id}-scopeQ`} label={t("entities.scopeQ")} description={t("entities.scopeQHint")}>
          <Input
            id={`${id}-scopeQ`}
            value={value.scopeQ ?? ""}
            placeholder="/scope/1"
            onChange={(event) => onChange({ ...value, scopeQ: event.target.value || undefined })}
          />
        </Field>
      </div>

      {value.type ? (
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1 text-caption text-fg-muted">{t("entities.filters")}</legend>
          {rows ? (
            rows.map((row, index) => {
              const slot = slots.find((s) => s.name === row.attribute);
              return (
                <div key={index} className="flex flex-wrap items-center gap-2">
                  {slots.length > 0 ? (
                    <Select
                      aria-label={t("entities.attribute")}
                      value={row.attribute}
                      onChange={(event) => setRow(index, { attribute: event.target.value, op: "==", value: "" })}
                    >
                      <option value="">—</option>
                      {slots.map((s) => (
                        <option key={s.name} value={s.name}>
                          {s.name}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      aria-label={t("entities.attribute")}
                      value={row.attribute}
                      className="w-40"
                      onChange={(event) => setRow(index, { attribute: event.target.value })}
                    />
                  )}
                  <Select
                    aria-label={t("entities.operator")}
                    value={row.op}
                    className="w-20"
                    onChange={(event) => setRow(index, { op: event.target.value as Filter["op"] })}
                  >
                    {opsFor(slot).map((op) => (
                      <option key={op} value={op}>
                        {op}
                      </option>
                    ))}
                  </Select>
                  <ValueInput slot={slot} value={row.value} onChange={(next) => setRow(index, { value: next })} />
                  <Button size="sm" variant="ghost" onClick={() => setRows(rows.filter((_, i) => i !== index))}>
                    {t("entities.remove")}
                  </Button>
                </div>
              );
            })
          ) : (
            <p className="text-caption text-fg-muted">{t("entities.advanced")}</p>
          )}
          {rows ? (
            <div>
              <Button
                size="sm"
                variant="secondary"
                onClick={() => setRows([...rows, { attribute: slots[0]?.name ?? "", op: "==", value: "" }])}
              >
                {t("entities.addFilter")}
              </Button>
            </div>
          ) : null}
          <Field id={`${id}-q`} label={t("entities.q")} description={t("entities.qHint")}>
            <Input
              id={`${id}-q`}
              value={q}
              className="font-mono"
              onChange={(event) => onChange({ ...value, q: event.target.value || undefined })}
            />
          </Field>
        </fieldset>
      ) : null}

      {value.type && slots.length > 0 ? (
        <fieldset className="flex flex-wrap gap-2">
          <legend className="mb-1 text-caption text-fg-muted">{t("entities.attrs")}</legend>
          {slots.map((slot) => (
            <label key={slot.name} className="inline-flex items-center gap-1 font-mono text-caption">
              <input
                type="checkbox"
                checked={(value.attrs ?? []).includes(slot.name)}
                onChange={(event) => toggleAttribute(slot.name, event.target.checked)}
              />
              {slot.name}
            </label>
          ))}
        </fieldset>
      ) : null}
    </div>
  );
}

const NUMERIC = ["integer", "float", "double", "decimal"];

/** The input the slot's range calls for: a number with its bounds, a choice, a URN, or text. */
function ValueInput({
  slot,
  value,
  onChange,
}: {
  slot: FilterSlot | undefined;
  value: string;
  onChange: (next: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const label = t("entities.value");
  if (slot?.range === "boolean") {
    return (
      <Select aria-label={label} value={value} className="w-28" onChange={(event) => onChange(event.target.value)}>
        <option value="">—</option>
        <option value="true">true</option>
        <option value="false">false</option>
      </Select>
    );
  }
  if (slot?.values) {
    return (
      <Select aria-label={label} value={value} className="w-40" onChange={(event) => onChange(event.target.value)}>
        <option value="">—</option>
        {slot.values.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </Select>
    );
  }
  if (NUMERIC.includes(slot?.range ?? "")) {
    return (
      <Input
        aria-label={label}
        type="number"
        value={value}
        min={slot?.minimum}
        max={slot?.maximum}
        step={slot?.range === "integer" ? 1 : "any"}
        className="w-32"
        onChange={(event) => onChange(event.target.value)}
      />
    );
  }
  const placeholder =
    slot?.kind === "Relationship"
      ? "urn:ngsi-ld:…"
      : slot?.range === "date"
        ? "2026-01-31"
        : slot?.range === "datetime"
          ? "2026-01-31T10:00:00Z"
          : undefined;
  return (
    <Input
      aria-label={label}
      value={value}
      placeholder={placeholder}
      className="w-48"
      onChange={(event) => onChange(event.target.value)}
    />
  );
}
