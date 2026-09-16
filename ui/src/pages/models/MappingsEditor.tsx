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
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import {
  Alert,
  Badge,
  Input,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Textarea,
} from "../../components/ui";

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
        <h2 id="mappings-pair" className="sr-only">
          {t("mappings.pair")}
        </h2>
        <label className="flex flex-col gap-1 text-body font-medium text-fg">
          {t("mappings.source")}
          <Select
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
          </Select>
        </label>
        <span aria-hidden="true" className="pb-2 text-lg text-fg-muted">
          →
        </span>
        <label className="flex flex-col gap-1 text-body font-medium text-fg">
          {t("mappings.target")}
          <Select
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
          </Select>
        </label>
        <div className="flex items-center gap-1.5 pb-1 text-body">
          <span className="text-fg-muted">{t("mappings.lane")}</span>
          <LifecycleBadge kind="lane" value={lane} />
        </div>
      </section>

      {missingRequired.length > 0 ? (
        <Alert role="alert" tone="danger">
          {t("mappings.unfilled", { slots: missingRequired.join(", ") })}
        </Alert>
      ) : null}

      <section aria-labelledby="mappings-canvas">
        <h2 id="mappings-canvas" className="text-title font-semibold text-fg">
          {t("mappings.canvas")}
        </h2>
        <Table caption={t("mappings.canvas")} className="mt-2">
          <TableHead>
            <TableHeaderCell>{t("mappings.targetSlot")}</TableHeaderCell>
            <TableHeaderCell>{t("mappings.sourceSlot")}</TableHeaderCell>
            <TableHeaderCell>{t("mappings.origin")}</TableHeaderCell>
            <TableHeaderCell>{t("mappings.unit")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {rows.map((row) => {
              const targetSlot = target.slots.find((slot) => slot.name === row.target);
              return (
                <TableRow key={row.target}>
                  <TableCell primary>
                    <span className="font-mono">
                      {row.target}
                      {targetSlot?.required ? <span aria-hidden="true"> *</span> : null}
                    </span>
                  </TableCell>
                  <TableCell>
                    <label className="sr-only" htmlFor={`from-${row.target}`}>
                      {t("mappings.sourceFor", { slot: row.target })}
                    </label>
                    <Select
                      id={`from-${row.target}`}
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
                    </Select>
                  </TableCell>
                  <TableCell>
                    {row.native?.trim() ? (
                      <Badge
                        tone="warning"
                        data-testid={`native-${row.target}`}
                      >
                        {t("mappings.native")}
                      </Badge>
                    ) : (
                      <span className="text-caption text-fg-muted">
                        {t(`mappings.originOf.${row.origin}`)}
                      </span>
                    )}
                  </TableCell>
                  <TableCell>
                    {row.unitConversion ? (
                      <span className="flex items-center gap-1">
                        <label className="sr-only" htmlFor={`unit-${row.target}`}>
                          {t("mappings.unitFor", { slot: row.target })}
                        </label>
                        <Select
                          id={`unit-${row.target}`}
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
                        </Select>
                        <label className="sr-only" htmlFor={`factor-${row.target}`}>
                          {t("mappings.factorFor", { slot: row.target })}
                        </label>
                        <Input
                          id={`factor-${row.target}`}
                          type="number"
                          step="any"
                          className="w-24"
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
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </section>

      <section aria-labelledby="mappings-golden" className="flex flex-col gap-2">
        <h2 id="mappings-golden" className="text-title font-semibold text-fg">
          {t("mappings.golden")}
        </h2>
        <label className="flex flex-col gap-1 text-body font-medium text-fg">
          {t("mappings.input")}
          <Textarea
            className="h-32 font-mono text-xs"
            value={example}
            onChange={(event) => setExample(event.target.value)}
          />
        </label>
        {parsedExample.error ? (
          <Alert role="alert" tone="danger">
            {parsedExample.error}
          </Alert>
        ) : (
          <output aria-label={t("mappings.output")} className="block">
            <pre className="overflow-x-auto rounded border border-border bg-surface-subtle p-2 font-mono text-xs text-fg">
              {JSON.stringify(result?.output ?? {}, null, 2)}
            </pre>
          </output>
        )}
        {result && result.unchecked.length > 0 ? (
          // DM-38: the browser cannot run Bloblang, and a preview that quietly omitted these
          // would read as "this slot is empty" rather than "nobody checked this slot".
          <p className="text-body">{t("mappings.unchecked", { slots: result.unchecked.join(", ") })}</p>
        ) : null}
        {result && result.missing.length > 0 ? (
          <p className="text-body text-fg-muted">
            {t("mappings.missing", { slots: result.missing.join(", ") })}
          </p>
        ) : null}
      </section>

      {blocks.length > 0 ? (
        <p className="text-body text-fg-muted">{t("mappings.laneRaised", { count: blocks.length })}</p>
      ) : null}
    </div>
  );
}

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
