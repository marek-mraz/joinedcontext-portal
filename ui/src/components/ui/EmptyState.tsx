import type { ReactNode } from "react";
import { clsx } from "clsx";
import { Icon } from "./icons";
import type { IconName } from "./icons";

export interface EmptyStateProps {
  title: ReactNode;
  /** One sentence on what would fill the page and how. */
  description?: ReactNode;
  icon?: IconName;
  /** The action that creates the first item, when the caller has one. */
  action?: ReactNode;
  /** Inside a table cell or a card the frame is already drawn; skip the dashed one. */
  bare?: boolean;
  className?: string;
}

/** What a list shows instead of nothing: why it is empty and what to do about it (UI-01). */
export function EmptyState({
  title,
  description,
  icon = "inbox",
  action,
  bare,
  className,
}: EmptyStateProps): React.JSX.Element {
  return (
    <div
      className={clsx(
        "flex flex-col items-center justify-center gap-2 px-6 py-12 text-center",
        !bare && "rounded-lg border border-dashed border-border-strong bg-surface",
        className,
      )}
    >
      <span className="mb-1 inline-flex size-11 items-center justify-center rounded-full bg-primary-soft text-primary-soft-fg">
        <Icon name={icon} className="size-5" />
      </span>
      <p className="text-body font-semibold text-fg">{title}</p>
      {description ? <p className="max-w-md text-body text-fg-muted">{description}</p> : null}
      {action ? <div className="mt-3">{action}</div> : null}
    </div>
  );
}
