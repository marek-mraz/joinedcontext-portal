import { forwardRef } from "react";
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { clsx } from "clsx";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "sm" | "md" | "lg";

const BASE =
  "focus-ring inline-flex shrink-0 select-none items-center justify-center gap-1.5 whitespace-nowrap rounded-md font-medium transition-colors disabled:pointer-events-none disabled:opacity-50";

const VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-primary text-primary-fg shadow-1 hover:bg-primary-hover active:bg-primary-active",
  secondary:
    "border border-border bg-surface text-fg shadow-1 hover:bg-surface-subtle active:bg-surface-muted",
  ghost: "text-fg hover:bg-surface-muted active:bg-surface-muted",
  danger: "bg-danger text-danger-fg shadow-1 hover:opacity-90 active:opacity-80",
};

const SIZES: Record<ButtonSize, string> = {
  sm: "h-8 px-2.5 text-caption",
  md: "h-9 px-3.5 text-body",
  lg: "h-11 px-5 text-body",
};

/** The classes of a button, for a `Link` or an `<a>` that has to look like one. */
export function buttonClass(
  variant: ButtonVariant = "secondary",
  size: ButtonSize = "md",
  className?: string,
): string {
  return clsx(BASE, VARIANTS[variant], SIZES[size], className);
}

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Drawn before the label; decorative, so pass an `aria-hidden` SVG. */
  icon?: ReactNode;
  /** Shows a spinner and disables the button; the label stays so the width does not jump. */
  loading?: boolean;
}

/** The one button of the Portal: a primary per page, secondaries around it (UI-01). */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", icon, loading = false, className, children, type, disabled, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? "button"}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={buttonClass(variant, size, className)}
      {...rest}
    >
      {loading ? <Spinner /> : icon}
      {children}
    </button>
  );
});

function Spinner(): React.JSX.Element {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      className="size-4 animate-spin"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.5"
      strokeLinecap="round"
    >
      <path d="M12 3a9 9 0 1 0 9 9" />
    </svg>
  );
}
