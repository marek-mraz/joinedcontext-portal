import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import {
  DEFAULT_KIND,
  NGSI_LD_KINDS,
  RANGES,
  UNIT_CODES,
  edit,
  parseModel,
  reservedNamespace,
  setOrDelete,
  slotAffordance,
  unitCode,
  UPSTREAM_ANNOTATION,
} from "./linkml";
import type { Diagnostic, LinkmlSlot, NgsiLdKind } from "./linkml";

/**
 * The structured view of the model: classes, slots, enums (DM-13).
 *
 * Every edit is applied to the YAML document and handed back as text, so this view and the
 * source view are two renderings of one document rather than two copies of it. Nothing is
 * mutated in place, and a source that does not parse leaves the view empty rather than
 * showing a stale tree.
 */
export interface LinkmlVisualEditorProps {
  source: string;
  onChange: (source: string) => void;
  diagnostics?: Diagnostic[];
  /** The organisation's configured locales, for the language maps of DM-15. */
  locales?: string[];
}

function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: JSX.Element;
  hint?: string;
}): JSX.Element {
  // The hint sits outside the label: inside it, it would become part of the field's
  // accessible name and a screen reader would read the whole sentence as the label.
  return (
    <div className="flex flex-col gap-1 text-sm">
      <label className="flex flex-col gap-1">
        <span className="font-medium">{label}</span>
        {children}
      </label>
      {hint ? <span className="text-xs text-surface-fg/70">{hint}</span> : null}
    </div>
  );
}

const INPUT =
  "rounded border border-border bg-surface px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus";

export function LinkmlVisualEditor({
  source,
  onChange,
  diagnostics = [],
  locales = [],
}: LinkmlVisualEditorProps): JSX.Element {
  const { t } = useTranslation();
  const model = useMemo(() => parseModel(source), [source]);

  const [selectedClass, setSelectedClass] = useState<string | null>(null);
  const [selectedSlot, setSelectedSlot] = useState<string | null>(null);
  const [newClass, setNewClass] = useState("");
  const [newSlot, setNewSlot] = useState("");
  const [newEnum, setNewEnum] = useState("");
  const [newValue, setNewValue] = useState("");
  const [iriError, setIriError] = useState<string | null>(null);
  // The IRI field keeps what was typed even while it is refused: a controlled input that
  // drops the keystroke would fight the person editing it.
  const [iriDraft, setIriDraft] = useState<string | null>(null);

  const activeClass =
    model.classes.find((klass) => klass.name === selectedClass) ?? model.classes[0];
  const activeSlot = model.slots.find((slot) => slot.name === selectedSlot);

  const apply = (mutate: Parameters<typeof edit>[1]) => onChange(edit(source, mutate));

  /** Opening another slot starts its IRI field from the document, not from the last draft. */
  const openSlot = (name: string) => {
    setSelectedSlot(name);
    setIriDraft(null);
    setIriError(null);
  };

  const addClass = () => {
    const name = newClass.trim();
    if (!name) {
      return;
    }
    apply((document) => document.setIn(["classes", name], { slots: [] }));
    setSelectedClass(name);
    setNewClass("");
  };

  const addSlot = () => {
    const name = newSlot.trim();
    if (!name || !activeClass) {
      return;
    }
    apply((document) => {
      document.setIn(["slots", name], { range: "string" });
      const existing = model.classes.find((klass) => klass.name === activeClass.name)?.slots ?? [];
      document.setIn(["classes", activeClass.name, "slots"], [...existing, name]);
    });
    openSlot(name);
    setNewSlot("");
  };

  const setSlotField = (name: string, path: (string | number)[], value: unknown) =>
    apply((document) => setOrDelete(document, ["slots", name, ...path], value));

  /**
   * A slot IRI is written only when it is the organisation's to mint (DM-16). A term under a
   * reserved namespace is refused here rather than flagged later, because the model would be
   * unpublishable and the IRI is exactly the thing consumers resolve.
   */
  const setSlotIri = (name: string, value: string) => {
    setIriDraft(value);
    const trimmed = value.trim();
    const slot = model.slots.find((candidate) => candidate.name === name);
    const reserved = trimmed ? reservedNamespace(trimmed, model.prefixes) : undefined;
    if (reserved && !slot?.upstream) {
      setIriError(t("models.iriReserved", { iri: trimmed, namespace: reserved }));
      return;
    }
    setIriError(null);
    setSlotField(name, ["slot_uri"], trimmed);
  };

  const setUnit = (name: string, code: string) => {
    const unit = UNIT_CODES.find((entry) => entry.code === code);
    apply((document) => {
      if (!unit) {
        document.deleteIn(["slots", name, "unit"]);
        return;
      }
      document.setIn(["slots", name, "unit"], {
        ucum_code: unit.ucum,
        exact_mappings: [`ucefact:${unit.code}`],
      });
    });
  };

  const setKind = (name: string, kind: NgsiLdKind) =>
    apply((document) =>
      setOrDelete(
        document,
        ["slots", name, "annotations", "ngsi_ld_kind"],
        kind === DEFAULT_KIND ? undefined : kind,
      ),
    );

  const setTitle = (path: (string | number)[], locale: string, phrase: string) =>
    apply((document) => setOrDelete(document, [...path, "title", locale], phrase));

  const addEnum = () => {
    const name = newEnum.trim();
    if (!name) {
      return;
    }
    apply((document) => document.setIn(["enums", name, "permissible_values"], {}));
    setNewEnum("");
  };

  const addEnumValue = (enumName: string) => {
    const value = newValue.trim();
    if (!value) {
      return;
    }
    apply((document) =>
      document.setIn(["enums", enumName, "permissible_values", value], { description: "" }),
    );
    setNewValue("");
  };

  const messagesFor = (path: string) =>
    diagnostics.filter((diagnostic) => diagnostic.path === path);

  return (
    <div className="grid gap-6 lg:grid-cols-[16rem_1fr]">
      <div className="flex flex-col gap-6">
        <section aria-labelledby="models-classes">
          <h3 id="models-classes" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.classes")}
          </h3>
          <ul className="flex flex-col gap-1">
            {model.classes.map((klass) => (
              <li key={klass.name}>
                <button
                  type="button"
                  onClick={() => setSelectedClass(klass.name)}
                  aria-current={klass.name === activeClass?.name ? "true" : undefined}
                  className={
                    klass.name === activeClass?.name
                      ? "w-full rounded bg-surface-subtle px-2 py-1 text-left text-sm font-medium"
                      : "w-full rounded px-2 py-1 text-left text-sm hover:bg-surface-subtle"
                  }
                >
                  {klass.name}
                </button>
              </li>
            ))}
          </ul>
          <div className="mt-2 flex gap-1">
            <input
              className={`${INPUT} w-full`}
              aria-label={t("models.newClass")}
              placeholder={t("models.newClass")}
              value={newClass}
              onChange={(event) => setNewClass(event.target.value)}
            />
            <button
              type="button"
              aria-label={t("models.addClass")}
              className="rounded border border-border px-2 text-sm"
              onClick={addClass}
            >
              {t("models.add")}
            </button>
          </div>
        </section>

        <section aria-labelledby="models-enums">
          <h3 id="models-enums" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.enums")}
          </h3>
          <ul className="flex flex-col gap-2">
            {model.enums.map((entry) => (
              <li key={entry.name} className="rounded border border-border p-2">
                <p className="text-sm font-medium">{entry.name}</p>
                <ul className="ml-3 list-disc text-xs text-surface-fg/80">
                  {entry.permissible_values.map((value) => (
                    <li key={value.name}>{value.name}</li>
                  ))}
                </ul>
                <div className="mt-1 flex gap-1">
                  <input
                    className={`${INPUT} w-full`}
                    aria-label={t("models.newEnumValue", { name: entry.name })}
                    value={newValue}
                    onChange={(event) => setNewValue(event.target.value)}
                  />
                  <button
                    type="button"
                    aria-label={t("models.addValue", { name: entry.name })}
                    className="rounded border border-border px-2 text-xs"
                    onClick={() => addEnumValue(entry.name)}
                  >
                    {t("models.add")}
                  </button>
                </div>
              </li>
            ))}
          </ul>
          <div className="mt-2 flex gap-1">
            <input
              className={`${INPUT} w-full`}
              aria-label={t("models.newEnum")}
              placeholder={t("models.newEnum")}
              value={newEnum}
              onChange={(event) => setNewEnum(event.target.value)}
            />
            <button
              type="button"
              aria-label={t("models.addEnum")}
              className="rounded border border-border px-2 text-sm"
              onClick={addEnum}
            >
              {t("models.add")}
            </button>
          </div>
        </section>
      </div>

      <div className="flex flex-col gap-6">
        {activeClass ? (
          <section aria-labelledby="models-class-detail" className="flex flex-col gap-3">
            <h3 id="models-class-detail" className="text-base font-semibold">
              {activeClass.name}
            </h3>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label={t("models.classUri")}>
                <input
                  className={INPUT}
                  value={activeClass.class_uri ?? ""}
                  onChange={(event) =>
                    apply((document) =>
                      setOrDelete(
                        document,
                        ["classes", activeClass.name, "class_uri"],
                        event.target.value.trim(),
                      ),
                    )
                  }
                />
              </Field>
              <Field label={t("models.description")}>
                <input
                  className={INPUT}
                  value={activeClass.description ?? ""}
                  onChange={(event) =>
                    apply((document) =>
                      setOrDelete(
                        document,
                        ["classes", activeClass.name, "description"],
                        event.target.value,
                      ),
                    )
                  }
                />
              </Field>
            </div>
            {locales.length > 0 ? (
              <div className="grid gap-3 sm:grid-cols-2">
                {locales.map((locale) => (
                  <Field key={locale} label={t("models.titleIn", { locale })}>
                    <input
                      className={INPUT}
                      value={activeClass.title?.[locale] ?? ""}
                      onChange={(event) =>
                        setTitle(["classes", activeClass.name], locale, event.target.value)
                      }
                    />
                  </Field>
                ))}
              </div>
            ) : null}
            {messagesFor(`classes.${activeClass.name}`).map((diagnostic) => (
              <p
                key={diagnostic.message}
                role="status"
                className="text-xs text-warning-fg"
              >
                {diagnostic.message}
              </p>
            ))}
          </section>
        ) : null}

        <section aria-labelledby="models-slots">
          <h3 id="models-slots" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.slots")}
          </h3>
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="text-xs uppercase text-surface-fg/70">
                <th scope="col" className="py-1">{t("models.slot")}</th>
                <th scope="col">{t("models.range")}</th>
                <th scope="col">{t("models.kind")}</th>
                <th scope="col">{t("models.unit")}</th>
                <th scope="col">{t("models.affordanceLabel")}</th>
              </tr>
            </thead>
            <tbody>
              {(activeClass?.slots ?? []).map((name) => {
                const slot: LinkmlSlot | undefined = model.slots.find(
                  (candidate) => candidate.name === name,
                );
                return (
                  <tr key={name} className="border-t border-border">
                    <th scope="row" className="py-1 font-normal">
                      <button
                        type="button"
                        className="underline-offset-2 hover:underline"
                        onClick={() => openSlot(name)}
                      >
                        {name}
                        {slot?.deprecated ? ` (${t("models.deprecated")})` : ""}
                      </button>
                    </th>
                    <td>{slot?.range ?? "—"}</td>
                    <td>{slot?.kind ?? DEFAULT_KIND}</td>
                    <td>{slot?.unit?.ucum_code ?? "—"}</td>
                    <td>{slot ? t(`models.affordance.${slotAffordance(slot)}`) : "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          <div className="mt-2 flex gap-1">
            <input
              className={`${INPUT} w-64`}
              aria-label={t("models.newSlot")}
              placeholder={t("models.newSlot")}
              value={newSlot}
              onChange={(event) => setNewSlot(event.target.value)}
            />
            <button
              type="button"
              aria-label={t("models.addSlot")}
              className="rounded border border-border px-2 text-sm"
              onClick={addSlot}
            >
              {t("models.add")}
            </button>
          </div>
        </section>

        {activeSlot ? (
          <section aria-labelledby="models-slot-detail" className="flex flex-col gap-3">
            <h3 id="models-slot-detail" className="text-base font-semibold">
              {activeSlot.name}
            </h3>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label={t("models.range")}>
                <select
                  className={INPUT}
                  value={activeSlot.range ?? ""}
                  onChange={(event) => setSlotField(activeSlot.name, ["range"], event.target.value)}
                >
                  <option value="">—</option>
                  {RANGES.map((range) => (
                    <option key={range} value={range}>
                      {range}
                    </option>
                  ))}
                  {model.enums.map((entry) => (
                    <option key={entry.name} value={entry.name}>
                      {entry.name}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label={t("models.kind")} hint={t("models.kindHint")}>
                <select
                  className={INPUT}
                  value={activeSlot.kind}
                  onChange={(event) => setKind(activeSlot.name, event.target.value as NgsiLdKind)}
                >
                  {NGSI_LD_KINDS.map((kind) => (
                    <option key={kind} value={kind}>
                      {kind}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label={t("models.slotUri")} hint={t("models.slotUriHint")}>
                <input
                  className={INPUT}
                  value={iriDraft ?? activeSlot.slot_uri ?? ""}
                  onChange={(event) => setSlotIri(activeSlot.name, event.target.value)}
                />
              </Field>
              <Field label={t("models.unit")} hint={t("models.unitHint")}>
                <select
                  className={INPUT}
                  value={unitCode(activeSlot.unit) ?? ""}
                  onChange={(event) => setUnit(activeSlot.name, event.target.value)}
                >
                  <option value="">—</option>
                  {UNIT_CODES.map((unit) => (
                    <option key={unit.code} value={unit.code}>
                      {`${unit.code} · ${unit.ucum} · ${unit.label}`}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
            <div className="flex flex-wrap gap-4 text-sm">
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={activeSlot.required === true}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, ["required"], event.target.checked)
                  }
                />
                {t("models.required")}
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={activeSlot.multivalued === true}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, ["multivalued"], event.target.checked)
                  }
                />
                {t("models.multivalued")}
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={activeSlot.deprecated === true}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, ["deprecated"], event.target.checked)
                  }
                />
                {t("models.deprecatedKeep")}
              </label>
            </div>
            {activeSlot.upstream ? (
              <p className="text-xs text-surface-fg/70">
                {t("models.upstream", { source: activeSlot.upstream })}
              </p>
            ) : null}
            {iriError ? (
              <p role="alert" className="text-sm text-danger-fg">
                {iriError}
              </p>
            ) : null}
            {messagesFor(`slots.${activeSlot.name}`).map((diagnostic) => (
              <p key={diagnostic.message} role="status" className="text-xs text-warning-fg">
                {diagnostic.message}
              </p>
            ))}
          </section>
        ) : null}
      </div>
    </div>
  );
}

export { UPSTREAM_ANNOTATION };
