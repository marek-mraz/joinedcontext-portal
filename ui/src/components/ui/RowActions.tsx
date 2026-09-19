import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "./Menu";
import { Button } from "./Button";

/** One thing a row can do, as the menu shows it. */
export interface RowAction {
  key: string;
  label: string;
  onSelect: () => void;
  tone?: "default" | "danger";
  /**
   * Why this action cannot be taken right now (UI-44). The item stays in the menu, disabled, and
   * says the reason — a person whose role is too narrow has to be able to read that, not guess it
   * from a missing line.
   */
  disabledReason?: string;
}

/**
 * The actions of one row: the obvious one in the open, the rest behind one menu at the end (T-2279,
 * UI-26).
 *
 * A row used to carry every action as its own button — open, edit, save as, work on a copy, delete —
 * so five controls competed with the data and the table could not be read down its columns. The menu
 * is Radix's, so the keyboard, the roles and Escape come from it; what this adds is the rule: one
 * action stays out, the others go in, and an action a person may not take is simply not in the list.
 *
 * A dialog must NOT be rendered inside the menu: the menu unmounts when it closes and would take the
 * dialog with it. Every action here only flips the row's own state, and the row renders the dialogs
 * beside the menu, which is what the `trigger={false}` of each action component is for.
 */
export function RowActions({
  label,
  primary,
  actions,
}: {
  /** What this row is, for the menu button's accessible name. */
  label: string;
  /** The action that stays in the open, if the row has an obvious one. */
  primary?: ReactNode;
  actions: RowAction[];
}): JSX.Element {
  const { t } = useTranslation();
  if (actions.length === 0) {
    return <>{primary}</>;
  }
  return (
    <div className="flex items-center justify-end gap-1.5">
      {primary}
      <Menu>
        <MenuTrigger asChild>
          <Button size="sm" variant="secondary" aria-label={t("rowActions.more", { name: label })}>
            ⋯
          </Button>
        </MenuTrigger>
        <MenuContent align="end">
          {actions.map((action) => (
            <MenuItem
              key={action.key}
              tone={action.tone}
              disabled={Boolean(action.disabledReason)}
              title={action.disabledReason}
              aria-disabled={action.disabledReason ? "true" : undefined}
              onSelect={action.disabledReason ? undefined : action.onSelect}
            >
              {action.label}
              {action.disabledReason ? (
                <span className="sr-only"> — {action.disabledReason}</span>
              ) : null}
            </MenuItem>
          ))}
        </MenuContent>
      </Menu>
    </div>
  );
}
