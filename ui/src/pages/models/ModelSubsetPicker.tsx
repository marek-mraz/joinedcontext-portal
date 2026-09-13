import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
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
        <ul role="alert" className="text-sm text-danger-fg">
          {problems.map((problem) => (
            <li key={problem}>{problem}</li>
          ))}
        </ul>
      ) : null}
      {subset.classes.length === 0 ? (
        <p role="status" className="text-sm text-warning-fg">
          {t("models.subset.nothing")}
        </p>
      ) : null}
      <ul className="flex flex-col gap-2">
        {model.classes.map((klass) => {
          const picked = chosen.get(klass.name);
          return (
            <li key={klass.name} className="rounded border border-border p-2">
              <label className="flex items-center gap-2 text-sm font-medium">
                <input
                  type="checkbox"
                  aria-label={klass.name}
                  checked={picked !== undefined}
                  onChange={(event) => onChange(toggleClass(subset, klass.name, event.target.checked))}
                />
                {klass.name}
              </label>
              <ul className="ml-6 mt-1 flex flex-wrap gap-x-4 gap-y-1">
                {klass.slots.map((slot) => {
                  const identity = IDENTITY_SLOTS.includes(slot);
                  return (
                    <li key={slot}>
                      <label className="flex items-center gap-1 text-sm">
                        <input
                          type="checkbox"
                          aria-label={`${klass.name}.${slot}`}
                          checked={identity ? picked !== undefined : (picked?.includes(slot) ?? false)}
                          disabled={identity}
                          onChange={(event) =>
                            onChange(toggleSlot(subset, klass.name, slot, event.target.checked))
                          }
                        />
                        <span className="font-mono text-xs">{slot}</span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            </li>
          );
        })}
      </ul>
      <p className="text-xs text-surface-fg/70">{t("models.subset.identity")}</p>
    </div>
  );
}
