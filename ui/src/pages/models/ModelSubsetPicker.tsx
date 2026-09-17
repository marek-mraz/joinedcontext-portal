import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Alert } from "../../components/ui";
import { effectiveSlots } from "./linkml";
import type { LinkmlModel } from "./linkml";
import { IDENTITY_SLOTS, subsetProblems, toggleClass, toggleSlot } from "./subset";
import type { Subset } from "./subset";

/**
 * The model as a tree of checkboxes: tick a class, tick the slots of it an endpoint exposes
 * (MP-01, UI-43). Identity slots are shown ticked and cannot be unticked, ticking nothing says
 * so, and a subset naming something the model lacks is reported rather than silently kept.
 */
export interface ModelSubsetPickerProps {
  model: LinkmlModel;
  subset: Subset;
  onChange: (subset: Subset) => void;
}

export function ModelSubsetPicker({ model, subset, onChange }: ModelSubsetPickerProps): JSX.Element {
  const { t } = useTranslation();
  const problems = useMemo(() => subsetProblems(model, subset), [model, subset]);
  const chosen = new Map(subset.classes.map((klass) => [klass.name, klass.slots]));

  return (
    <div className="flex flex-col gap-3">
      {problems.length > 0 ? (
        <Alert role="alert" tone="danger">
          <ul className="flex flex-col gap-1">
            {problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {subset.classes.length === 0 ? (
        <Alert role="status" tone="warning">
          {t("models.subset.nothing")}
        </Alert>
      ) : null}
      <ul className="flex flex-col gap-2">
        {model.classes.map((klass) => {
          const picked = chosen.get(klass.name);
          return (
            <li key={klass.name} className="rounded-lg border border-border bg-surface p-3">
              <label className="flex items-center gap-2 text-body font-medium text-fg">
                <input
                  type="checkbox"
                  aria-label={klass.name}
                  checked={picked !== undefined}
                  onChange={(event) => onChange(toggleClass(subset, klass.name, event.target.checked))}
                />
                {klass.name}
              </label>
              <ul className="ml-6 mt-2 flex flex-wrap gap-x-4 gap-y-1.5">
                {effectiveSlots(model, klass).map((slot) => {
                  const identity = IDENTITY_SLOTS.includes(slot);
                  return (
                    <li key={slot}>
                      <label className="flex items-center gap-1.5 text-body">
                        <input
                          type="checkbox"
                          aria-label={`${klass.name}.${slot}`}
                          checked={identity ? picked !== undefined : (picked?.includes(slot) ?? false)}
                          disabled={identity}
                          onChange={(event) =>
                            onChange(toggleSlot(subset, klass.name, slot, event.target.checked))
                          }
                        />
                        <span className="font-mono text-caption text-fg">{slot}</span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            </li>
          );
        })}
      </ul>
      <p className="text-caption text-fg-muted">{t("models.subset.identity")}</p>
    </div>
  );
}
