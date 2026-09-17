import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import {
  Alert,
  Button,
  Field,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import {
  DEFAULT_KIND,
  NGSI_LD_KINDS,
  RANGES,
  UNIT_CODES,
  parseModel,
  reservedNamespace,
  slotAffordance,
  unitCode,
  UPSTREAM_ANNOTATION,
} from "./linkml";
import type { Diagnostic, LinkmlSlot, NgsiLdKind } from "./linkml";
import { applyOperations } from "./operations";
import type { Operation, SlotField } from "./operations";

/**
 * The structured view of the model: classes, slots, enums (DM-13).
 *
 * Every edit is one `Operation` applied to the YAML document and handed back as text, so this
 * view and the source view are two renderings of one document rather than two copies of it,
 * and an assistant editing through `applyOperations` does exactly what a click here does
 * (DM-31). Nothing is mutated in place, and a source that does not parse leaves the view empty
 * rather than showing a stale tree.
 */
export interface LinkmlVisualEditorProps {
  source: string;
  onChange: (source: string) => void;
  diagnostics?: Diagnostic[];
  /** The organisation's configured locales, for the language maps of DM-15. */
  locales?: string[];
}

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
  const [refusal, setRefusal] = useState<string | null>(null);

  const activeClass =
    model.classes.find((klass) => klass.name === selectedClass) ?? model.classes[0];
  const activeSlot = model.slots.find((slot) => slot.name === selectedSlot);

  /** One operation, the same way an assistant would send it; a refusal is shown, not swallowed. */
  const run = (operation: Operation): boolean => {
    const applied = applyOperations(source, [operation]);
    setRefusal(applied.refused[0]?.reason ?? null);
    if (applied.refused.length === 0) {
      onChange(applied.source);
    }
    return applied.refused.length === 0;
  };

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
    if (run({ op: "addClass", name })) {
      setSelectedClass(name);
      setNewClass("");
    }
  };

  const addSlot = () => {
    const name = newSlot.trim();
    if (!name || !activeClass) {
      return;
    }
    if (run({ op: "addSlot", name, class: activeClass.name })) {
      openSlot(name);
      setNewSlot("");
    }
  };

  const setSlotField = (name: string, field: SlotField, value: unknown) =>
    run({ op: "setSlot", name, field, value });

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
    setSlotField(name, "slot_uri", trimmed);
  };

  const setTitle = (target: "class" | "slot", name: string, locale: string, value: string) =>
    run({ op: "setTitle", target, name, locale, value });

  const addEnum = () => {
    const name = newEnum.trim();
    if (!name) {
      return;
    }
    if (run({ op: "addEnum", name })) {
      setNewEnum("");
    }
  };

  const addEnumValue = (enumName: string) => {
    const value = newValue.trim();
    if (!value) {
      return;
    }
    if (run({ op: "addEnumValue", enum: enumName, value })) {
      setNewValue("");
    }
  };

  const messagesFor = (path: string) =>
    diagnostics.filter((diagnostic) => diagnostic.path === path);

  return (
    <div className="grid gap-6 lg:grid-cols-[16rem_1fr]">
      {refusal ? (
        <Alert tone="danger" role="alert" className="lg:col-span-2">
          {t("models.refused", { reason: refusal })}
        </Alert>
      ) : null}
      <div className="flex flex-col gap-6">
        <section aria-labelledby="models-classes">
          <h2 id="models-classes" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.classes")}
          </h2>
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
            <Input
              aria-label={t("models.newClass")}
              placeholder={t("models.newClass")}
              value={newClass}
              onChange={(event) => setNewClass(event.target.value)}
            />
            <Button
              size="sm"
              aria-label={t("models.addClass")}
              onClick={addClass}
            >
              {t("models.add")}
            </Button>
          </div>
        </section>

        <section aria-labelledby="models-enums">
          <h2 id="models-enums" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.enums")}
          </h2>
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
                  <Input
                    aria-label={t("models.newEnumValue", { name: entry.name })}
                    value={newValue}
                    onChange={(event) => setNewValue(event.target.value)}
                  />
                  <Button
                    size="sm"
                    aria-label={t("models.addValue", { name: entry.name })}
                    onClick={() => addEnumValue(entry.name)}
                  >
                    {t("models.add")}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
          <div className="mt-2 flex gap-1">
            <Input
              aria-label={t("models.newEnum")}
              placeholder={t("models.newEnum")}
              value={newEnum}
              onChange={(event) => setNewEnum(event.target.value)}
            />
            <Button
              size="sm"
              aria-label={t("models.addEnum")}
              onClick={addEnum}
            >
              {t("models.add")}
            </Button>
          </div>
        </section>
      </div>

      <div className="flex flex-col gap-6">
        {activeClass ? (
          <section aria-labelledby="models-class-detail" className="flex flex-col gap-3">
            <h2 id="models-class-detail" className="text-base font-semibold">
              {activeClass.name}
            </h2>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field id="class-uri" label={t("models.classUri")}>
                <Input
                  id="class-uri"
                  value={activeClass.class_uri ?? ""}
                  onChange={(event) =>
                    run({
                      op: "setClass",
                      name: activeClass.name,
                      field: "class_uri",
                      value: event.target.value,
                    })
                  }
                />
              </Field>
              {/* T-1088, DM-13: the hierarchy the model declares, editable where it is read.
                  A parent can only be another class of this model, so it is a choice, not a
                  free text a typo turns into a dangling reference. */}
              <Field id="class-parent" label={t("models.classParent")}>
                <select
                  id="class-parent"
                  className="focus-ring w-full rounded border border-border bg-surface px-2 py-1 text-sm"
                  value={activeClass.is_a ?? ""}
                  onChange={(event) =>
                    run({
                      op: "setClass",
                      name: activeClass.name,
                      field: "is_a",
                      value: event.target.value,
                    })
                  }
                >
                  <option value="">{t("models.classParentNone")}</option>
                  {model.classes
                    .filter((other) => other.name !== activeClass.name)
                    .map((other) => (
                      <option key={other.name} value={other.name}>
                        {other.name}
                      </option>
                    ))}
                </select>
              </Field>
              <Field id="class-mixins" label={t("models.classMixins")}>
                <Input
                  id="class-mixins"
                  value={(activeClass.mixins ?? []).join(", ")}
                  onChange={(event) =>
                    run({
                      op: "setClassMixins",
                      name: activeClass.name,
                      mixins: event.target.value.split(","),
                    })
                  }
                />
              </Field>
              <Field id="class-description" label={t("models.description")}>
                <Input
                  id="class-description"
                  value={activeClass.description ?? ""}
                  onChange={(event) =>
                    run({
                      op: "setClass",
                      name: activeClass.name,
                      field: "description",
                      value: event.target.value,
                    })
                  }
                />
              </Field>
            </div>
            {locales.length > 0 ? (
              <div className="grid gap-3 sm:grid-cols-2">
                {locales.map((locale) => (
                  <Field key={locale} id={`class-title-${locale}`} label={t("models.titleIn", { locale })}>
                    <Input
                      id={`class-title-${locale}`}
                      value={activeClass.title?.[locale] ?? ""}
                      onChange={(event) =>
                        setTitle("class", activeClass.name, locale, event.target.value)
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
          <h2 id="models-slots" className="mb-2 text-sm font-semibold uppercase tracking-wide">
            {t("models.slots")}
          </h2>
          <Table caption={t("models.slots")}>
            <TableHead>
              <TableHeaderCell>{t("models.slot")}</TableHeaderCell>
              <TableHeaderCell>{t("models.range")}</TableHeaderCell>
              <TableHeaderCell>{t("models.kind")}</TableHeaderCell>
              <TableHeaderCell>{t("models.unit")}</TableHeaderCell>
              <TableHeaderCell>{t("models.affordanceLabel")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {(activeClass?.slots ?? []).map((name) => {
                const slot: LinkmlSlot | undefined = model.slots.find(
                  (candidate) => candidate.name === name,
                );
                return (
                  <TableRow key={name}>
                    <TableCell primary>
                      <button
                        type="button"
                        className="underline-offset-2 hover:underline"
                        onClick={() => openSlot(name)}
                      >
                        {name}
                        {slot?.deprecated ? ` (${t("models.deprecated")})` : ""}
                      </button>
                    </TableCell>
                    <TableCell>{slot?.range ?? "—"}</TableCell>
                    <TableCell>{slot?.kind ?? DEFAULT_KIND}</TableCell>
                    <TableCell>{slot?.unit?.ucum_code ?? "—"}</TableCell>
                    <TableCell>{slot ? t(`models.affordance.${slotAffordance(slot)}`) : "—"}</TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
          <div className="mt-2 flex gap-1">
            <Input
              className="w-64"
              aria-label={t("models.newSlot")}
              placeholder={t("models.newSlot")}
              value={newSlot}
              onChange={(event) => setNewSlot(event.target.value)}
            />
            <Button
              size="sm"
              aria-label={t("models.addSlot")}
              onClick={addSlot}
            >
              {t("models.add")}
            </Button>
          </div>
        </section>

        {activeSlot ? (
          <section aria-labelledby="models-slot-detail" className="flex flex-col gap-3">
            <h2 id="models-slot-detail" className="text-base font-semibold">
              {activeSlot.name}
            </h2>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field id="slot-range" label={t("models.range")}>
                <Select
                  id="slot-range"
                  value={activeSlot.range ?? ""}
                  onChange={(event) => setSlotField(activeSlot.name, "range", event.target.value)}
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
                </Select>
              </Field>
              <Field id="slot-kind" label={t("models.kind")} help={t("models.kindHint")}>
                <Select
                  id="slot-kind"
                  value={activeSlot.kind}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, "kind", event.target.value as NgsiLdKind)
                  }
                >
                  {NGSI_LD_KINDS.map((kind) => (
                    <option key={kind} value={kind}>
                      {kind}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field id="slot-uri" label={t("models.slotUri")} help={t("models.slotUriHint")}>
                <Input
                  id="slot-uri"
                  value={iriDraft ?? activeSlot.slot_uri ?? ""}
                  onChange={(event) => setSlotIri(activeSlot.name, event.target.value)}
                />
              </Field>
              {/* T-1088: which profiles take this slot, beside the rest of what it declares. */}
              <Field id="slot-subsets" label={t("models.slotSubsets")}>
                <Input
                  id="slot-subsets"
                  value={(activeSlot.subsets ?? []).join(", ")}
                  onChange={(event) =>
                    run({
                      op: "setSlotSubsets",
                      name: activeSlot.name,
                      subsets: event.target.value.split(","),
                    })
                  }
                />
              </Field>
              <Field id="slot-unit" label={t("models.unit")} help={t("models.unitHint")}>
                <Select
                  id="slot-unit"
                  value={unitCode(activeSlot.unit) ?? ""}
                  onChange={(event) => setSlotField(activeSlot.name, "unit", event.target.value)}
                >
                  <option value="">—</option>
                  {UNIT_CODES.map((unit) => (
                    <option key={unit.code} value={unit.code}>
                      {`${unit.code} · ${unit.ucum} · ${unit.label}`}
                    </option>
                  ))}
                </Select>
              </Field>
            </div>
            <div className="flex flex-wrap gap-4 text-sm">
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={activeSlot.required === true}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, "required", event.target.checked)
                  }
                />
                {t("models.required")}
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={activeSlot.multivalued === true}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, "multivalued", event.target.checked)
                  }
                />
                {t("models.multivalued")}
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={activeSlot.deprecated === true}
                  onChange={(event) =>
                    setSlotField(activeSlot.name, "deprecated", event.target.checked)
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
              <Alert tone="danger" role="alert">
                {iriError}
              </Alert>
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
