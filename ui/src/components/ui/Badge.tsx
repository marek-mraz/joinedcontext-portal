import type { HTMLAttributes } from "react";
import { clsx } from "clsx";

export type BadgeTone = "neutral" | "primary" | "success" | "warning" | "danger" | "info" | "purple";

const TONES: Record<BadgeTone, string> = {
  neutral: "border-border bg-surface-subtle text-fg-muted",
  primary: "border-primary-200 bg-primary-soft text-primary-soft-fg",
  success: "border-success/30 bg-success-soft text-success",
  warning: "border-warning/30 bg-warning-soft text-warning",
  danger: "border-danger/30 bg-danger-soft text-danger",
  info: "border-info/30 bg-info-soft text-info",
  purple: "border-purple-500/40 bg-purple-500/15 text-fg",
};

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: BadgeTone;
  /** Monospace: for a slug, a representation name, a secret reference. */
  mono?: boolean;
}

/** A small labelled chip; the tone is never the only carrier of meaning, the text is. */
export function Badge({
  tone = "neutral",
  mono,
  className,
  children,
  ...rest
}: BadgeProps): React.JSX.Element {
  return (
    <span
      className={clsx(
        "inline-flex items-center gap-1 whitespace-nowrap rounded-full border px-2 py-0.5 text-caption font-medium",
        mono && "font-mono",
        TONES[tone],
        className,
      )}
      {...rest}
    >
      {children}
    </span>
  );
}
