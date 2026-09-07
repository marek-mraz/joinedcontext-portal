import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { UNIT_CODES, parseModel } from "./linkml";
import type { LinkmlModel } from "./linkml";
import {
  autoAlign,
  laneOf,
  nativeBlocks,
  toTransformationSpec,
  transform,
  unfilledRequired,
} from "./mapping";
import type { Alignment, Derivation } from "./mapping";

/**
 * The Mappings tab: two models side by side and the derivations between them (T-0223, DM-33,
 * DM-38, DM-40).
 *
 * The editor's job is to make a `TransformationSpecification` that a person can review. It
 * never invents a derivation it cannot justify — every filled row says where its guess came
 * from — and it never claims a mapping is correct: the browser runs the invertible subset over
 * one example so the author can see the shape, and CI runs the golden tests through Bento and
 * Model Tools, which is the answer that counts (DM-39, DM-52).
 *
 * Nothing here writes to the platform. Saving a Mapping is a repository change and travels the
 * lane like any other, which is why the lane is on screen while the mapping is being written
 * rather than after it is submitted (DM-38, CC-32).
 */

/** One model the project publishes, as this editor needs it. */
export interface MappingModel {
  name: string;
  version: string;
  /** The LinkML source, which is the one place the slots come from. */
  source: string;
}

export interface MappingsEditorProps {
  /** The models to map between; two are needed before anything can be aligned. */
  models: MappingModel[];
  /** SSSOM alignment sets, used only to pre-fill (DM-42). */
  alignments?: Alignment[];
  /** Called whenever the specification changes, for the page that saves it. */
  onChange?: (spec: string) => void;
}

const EXAMPLE = `{\n  "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:st-1",\n  "type": "AirQualityObserved"\n}`;

export function MappingsEditor({
  models,
  alignments = [],
  onChange,
}: MappingsEditorProps): JSX.Element {
  const { t } = useTranslation();
  const [sourceName, setSourceName] = useState(models[0]?.name ?? "");
  const [targetName, setTargetName] = useState(models[1]?.name ?? models[0]?.name ?? "");
  const [derivations, setDerivations] = useState<Derivation[] | undefined>(undefined);
  const [example, setExample] = useState(EXAMPLE);

  const source = useParsed(models, sourceName);
  const target = useParsed(models, targetName);

  // The alignment is recomputed whenever the pair changes, and the author's own edits are kept
  // only while the pair does not: a derivation onto a slot the new target does not have is a
  // derivation onto nothing.
  const aligned = useMemo(
    () => autoAlign(source, target, alignments),
    [source, target, alignments],
  );
  const rows = derivations ?? aligned;

  const update = (targetSlot: string, change: Partial<Derivation>) => {
    const next = rows.map((row) =>
      row.target === targetSlot ? { ...row, ...change, origin: "manual" as const } : row,
    );
    setDerivations(next);
    onChange?.(toTransformationSpec(next, className(source), className(target)));
  };

  const missingRequired = unfilledRequired(target, rows);
  const lane = laneOf(rows);
  const blocks = nativeBlocks(rows);

  const parsedExample = useMemo(() => {
    try {
      const value: unknown = JSON.parse(example);
      return typeof value === "object" && value !== null
        ? { value: value as Record<string, unknown> }
        : { error: t("mappings.exampleNotAnObject") };
    } catch {
      return { error: t("mappings.exampleNotJson") };
    }
  }, [example, t]);
  const result = parsedExample.value ? transform(parsedExample.value, rows) : undefined;

  return (
    <div className="flex flex-col gap-4">
      <section aria-labelledby="mappings-pair" className="flex flex-wrap items-end gap-3">
        <h3 id="mappings-pair" className="sr-only">
          {t("mappings.pair")}
        </h3>
        <label className="flex flex-col gap-1 text-sm">
          {t("mappings.source")}
          <select
            className="rounded border border-border px-2 py-1"
            value={sourceName}
            onChange={(event) => {
              setSourceName(event.target.value);
              setDerivations(undefined);
            }}
          >
            {models.map((model) => (
              <option key={model.name} value={model.name}>
                {model.name} {model.version}
              </option>
            ))}
          </select>
        </label>
        <span aria-hidden="true" className="pb-2 text-lg">
          →
        </span>
        <label className="flex flex-col gap-1 text-sm">
          {t("mappings.target")}
          <select
            className="rounded border border-border px-2 py-1"
            value={targetName}
            onChange={(event) => {
              setTargetName(event.target.value);
              setDerivations(undefined);
            }}
          >
            {models.map((model) => (
              <option key={model.name} value={model.name}>
                {model.name} {model.version}
              </option>
            ))}
          </select>
        </label>
        <p className="pb-1 text-sm">
          {t("mappings.lane")} <span className={`font-semibold ${LANE[lane]}`}>{t(`lane.${lane}`)}</span>
        </p>
      </section>

      {missingRequired.length > 0 ? (
        <p role="alert" className="rounded border border-danger p-2 text-sm text-danger-fg">
          {t("mappings.unfilled", { slots: missingRequired.join(", ") })}
        </p>
      ) : null}

      <section aria-labelledby="mappings-canvas">
        <h3 id="mappings-canvas" className="text-sm font-semibold">
          {t("mappings.canvas")}
        </h3>
        <table className="mt-2 w-full text-sm">
          <thead>
            <tr className="text-left">
              <th scope="col">{t("mappings.targetSlot")}</th>
              <th scope="col">{t("mappings.sourceSlot")}</th>
              <th scope="col">{t("mappings.origin")}</th>
              <th scope="col">{t("mappings.unit")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const targetSlot = target.slots.find((slot) => slot.name === row.target);
              return (
                <tr key={row.target} className="border-t border-border">
                  <th scope="row" className="py-1 text-left font-mono font-normal">
                    {row.target}
                    {targetSlot?.required ? <span aria-hidden="true"> *</span> : null}
                  </th>
                  <td className="py-1">
                    <label className="sr-only" htmlFor={`from-${row.target}`}>
                      {t("mappings.sourceFor", { slot: row.target })}
                    </label>
                    <select
                      id={`from-${row.target}`}
                      className="rounded border border-border px-1"
                      value={row.populatedFrom ?? ""}
                      onChange={(event) =>
                        update(row.target, { populatedFrom: event.target.value || undefined })
                      }
                    >
                      <option value="">{t("mappings.unmapped")}</option>
                      {source.slots.map((slot) => (
                        <option key={slot.name} value={slot.name}>
                          {slot.name}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td className="py-1">
                    {row.native?.trim() ? (
                      <span
                        data-testid={`native-${row.target}`}
                        className="rounded bg-warning px-1 text-xs font-semibold text-warning-fg"
                      >
                        {t("mappings.native")}
                      </span>
                    ) : (
                      <span className="text-xs text-surface-fg/70">
                        {t(`mappings.originOf.${row.origin}`)}
                      </span>
                    )}
                  </td>
                  <td className="py-1">
                    {row.unitConversion ? (
                      <span className="flex items-center gap-1">
                        <label className="sr-only" htmlFor={`unit-${row.target}`}>
                          {t("mappings.unitFor", { slot: row.target })}
                        </label>
                        <select
                          id={`unit-${row.target}`}
                          className="rounded border border-border px-1"
                          value={row.unitConversion.toUnit ?? ""}
                          onChange={(event) =>
                            update(row.target, {
                              unitConversion: {
                                ...row.unitConversion,
                                factor: row.unitConversion?.factor ?? 1,
                                toUnit: event.target.value,
                              },
                            })
                          }
                        >
                          {UNIT_CODES.map((code) => (
                            <option key={code.code} value={code.code}>
                              {code.code} — {code.label}
                            </option>
                          ))}
                        </select>
                        <label className="sr-only" htmlFor={`factor-${row.target}`}>
                          {t("mappings.factorFor", { slot: row.target })}
                        </label>
                        <input
                          id={`factor-${row.target}`}
                          type="number"
                          step="any"
                          className="w-20 rounded border border-border px-1"
                          value={row.unitConversion.factor}
                          onChange={(event) =>
                            update(row.target, {
                              unitConversion: {
                                ...row.unitConversion,
                                factor: Number(event.target.value),
                                toUnit: row.unitConversion?.toUnit,
                              },
                            })
                          }
                        />
                      </span>
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </section>

      <section aria-labelledby="mappings-golden" className="flex flex-col gap-2">
        <h3 id="mappings-golden" className="text-sm font-semibold">
          {t("mappings.golden")}
        </h3>
        <label className="flex flex-col gap-1 text-sm">
          {t("mappings.input")}
          <textarea
            className="h-32 rounded border border-border p-2 font-mono text-xs"
            value={example}
            onChange={(event) => setExample(event.target.value)}
          />
        </label>
        {parsedExample.error ? (
          <p role="alert" className="text-sm text-danger-fg">
            {parsedExample.error}
          </p>
        ) : (
          <output aria-label={t("mappings.output")} className="block">
            <pre className="overflow-x-auto rounded border border-border bg-surface-subtle p-2 font-mono text-xs">
              {JSON.stringify(result?.output ?? {}, null, 2)}
            </pre>
          </output>
        )}
        {result && result.unchecked.length > 0 ? (
          // DM-38: the browser cannot run Bloblang, and a preview that quietly omitted these
          // would read as "this slot is empty" rather than "nobody checked this slot".
          <p className="text-sm">{t("mappings.unchecked", { slots: result.unchecked.join(", ") })}</p>
        ) : null}
        {result && result.missing.length > 0 ? (
          <p className="text-sm text-surface-fg/70">
            {t("mappings.missing", { slots: result.missing.join(", ") })}
          </p>
        ) : null}
      </section>

      {blocks.length > 0 ? (
        <p className="text-sm">{t("mappings.laneRaised", { count: blocks.length })}</p>
      ) : null}
    </div>
  );
}

const LANE: Record<string, string> = {
  green: "text-success-fg",
  yellow: "text-warning-fg",
  red: "text-danger-fg",
};

/** The named model, parsed; an empty model when nothing is selected yet. */
function useParsed(models: MappingModel[], name: string): LinkmlModel {
  return useMemo(() => {
    const model = models.find((candidate) => candidate.name === name);
    return parseModel(model?.source ?? "");
  }, [models, name]);
}

/** The one class a model declares, which is what a class derivation is written against. */
function className(model: LinkmlModel): string {
  return model.classes[0]?.name ?? model.name ?? "Entity";
}
