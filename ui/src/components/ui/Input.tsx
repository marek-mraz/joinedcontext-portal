import { forwardRef } from "react";
import type { InputHTMLAttributes, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { clsx } from "clsx";

/** What every text control shares: the height of a button, the border of a card, one ring. */
export const CONTROL =
  "focus-ring w-full rounded-md border border-border bg-surface text-body text-fg shadow-1 transition-colors placeholder:text-fg-subtle hover:border-border-strong focus-visible:border-ring disabled:cursor-not-allowed disabled:bg-surface-muted disabled:opacity-60 aria-[invalid=true]:border-danger read-only:bg-surface-subtle";

export type InputProps = InputHTMLAttributes<HTMLInputElement>;

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, type, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      type={type ?? "text"}
      className={clsx(CONTROL, "h-9 px-3", type === "number" && "tabular-nums", className)}
      {...rest}
    />
  );
});

export type TextareaProps = TextareaHTMLAttributes<HTMLTextAreaElement>;

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(function Textarea(
  { className, rows, ...rest },
  ref,
) {
  return (
    <textarea
      ref={ref}
      rows={rows ?? 4}
      className={clsx(CONTROL, "min-h-9 px-3 py-2 leading-relaxed", className)}
      {...rest}
    />
  );
});

export type SelectProps = SelectHTMLAttributes<HTMLSelectElement>;

/**
 * A native `<select>`, styled: it is the one control the keyboard, the screen reader and the
 * phone all already know, and a JSON Schema enum is exactly what it holds.
 */
export const Select = forwardRef<HTMLSelectElement, SelectProps>(function Select(
  { className, multiple, children, ...rest },
  ref,
) {
  return (
    <span className={clsx("relative block", multiple && "[&>svg]:hidden")}>
      <select
        ref={ref}
        multiple={multiple}
        className={clsx(
          CONTROL,
          multiple ? "min-h-24 px-3 py-2" : "h-9 appearance-none pl-3 pr-9",
          className,
        )}
        {...rest}
      >
        {children}
      </select>
      <svg
        aria-hidden="true"
        viewBox="0 0 20 20"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-fg-subtle"
      >
        <path d="m6 8 4 4 4-4" />
      </svg>
    </span>
  );
});
