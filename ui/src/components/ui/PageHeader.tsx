import type { ReactNode } from "react";
import { clsx } from "clsx";

export interface PageHeaderProps {
  title: ReactNode;
  /** One line under the title on what the page manages. */
  description?: ReactNode;
  /** The breadcrumb trail, rendered above the title; the shell provides it. */
  breadcrumb?: ReactNode;
  /** The page's actions; the first is the only primary on the page. */
  actions?: ReactNode;
  /** Anything that sits beside the actions: a quota bar, a filter. */
  aside?: ReactNode;
  className?: string;
}

/** The top of every page: what it is, where it is, and what one can do on it (UI-01). */
export function PageHeader({
  title,
  description,
  breadcrumb,
  actions,
  aside,
  className,
}: PageHeaderProps): React.JSX.Element {
  // A div, not a <header>: the shell's top bar is the one banner landmark of the page.
  return (
    <div className={clsx("flex flex-col gap-3", className)}>
      {breadcrumb}
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
        <div className="min-w-0">
          <h1 className="text-display font-semibold tracking-tight text-fg">{title}</h1>
          {description ? <p className="mt-1 max-w-prose text-body text-fg-muted">{description}</p> : null}
        </div>
        {actions || aside ? (
          <div className="flex flex-wrap items-end gap-x-6 gap-y-3">
            {aside}
            {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}
