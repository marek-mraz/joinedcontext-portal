import type { HTMLAttributes, ReactNode } from "react";
import { clsx } from "clsx";
import { Icon } from "./icons";
import type { IconName } from "./icons";

export type AlertTone = "info" | "success" | "warning" | "danger";

const TONES: Record<AlertTone, { box: string; icon: IconName }> = {
  info: { box: "border-info/30 bg-info-soft text-fg", icon: "info" },
  success: { box: "border-success/30 bg-success-soft text-fg", icon: "check" },
  warning: { box: "border-warning/30 bg-warning-soft text-fg", icon: "warning" },
  danger: { box: "border-danger/30 bg-danger-soft text-fg", icon: "error" },
};

export interface AlertProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  tone?: AlertTone;
  title?: ReactNode;
  /** Buttons or links that answer the message: a retry, a review link. */
  actions?: ReactNode;
}

/**
 * An inline message with the tone in its icon and its text, never in colour alone. The
 * caller sets the `role`: `alert` for an error that just happened, `status` for a notice.
 */
export function Alert({
  tone = "info",
  title,
  actions,
  className,
  children,
  ...rest
}: AlertProps): React.JSX.Element {
  const { box, icon } = TONES[tone];
  return (
    <div
      className={clsx("flex gap-3 rounded-lg border px-4 py-3 text-body", box, className)}
      {...rest}
    >
      <Icon
        name={icon}
        className={clsx(
          "mt-0.5 size-4",
          tone === "danger" && "text-danger",
          tone === "warning" && "text-warning",
          tone === "success" && "text-success",
          tone === "info" && "text-info",
        )}
      />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        {title ? <p className="font-semibold">{title}</p> : null}
        {children ? <div className="[&_a]:underline">{children}</div> : null}
        {actions ? <div className="mt-1 flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    </div>
  );
}
