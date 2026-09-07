import type { ReactNode } from "react";
import { clsx } from "clsx";

export interface FieldProps {
  /** The id of the control inside; the label points at it and the messages hang off it. */
  id: string;
  label?: ReactNode;
  required?: boolean;
  /** One sentence on what the value is for; read before the control. */
  description?: ReactNode;
  /** A hint shown under the control. */
  help?: ReactNode;
  /** The messages of a failed validation, one per line. */
  errors?: string[];
  /** A label-less control (a checkbox carries its own) still gets the messages. */
  hideLabel?: boolean;
  className?: string;
  children: ReactNode;
}

/** The ids a Field gives its messages, so a control can name them in `aria-describedby`. */
export function fieldIds(id: string) {
  return {
    description: `${id}__description`,
    help: `${id}__help`,
    error: `${id}__error`,
  };
}

/**
 * Label, description, control, help and errors, in that order, with the ids wired so a
 * screen reader hears all of it (UI-01). The control is the caller's: an Input, a Select, a
 * widget of rjsf, whatever the form needs.
 */
export function Field({
  id,
  label,
  required,
  description,
  help,
  errors,
  hideLabel,
  className,
  children,
}: FieldProps): React.JSX.Element {
  const ids = fieldIds(id);
  const hasErrors = Boolean(errors && errors.length > 0);
  return (
    <div
      data-invalid={hasErrors ? "true" : undefined}
      className={clsx("flex flex-col gap-1.5", className)}
    >
      {label && !hideLabel ? (
        <label htmlFor={id} className="text-body font-medium text-fg">
          {label}
          {required ? (
            <span aria-hidden="true" className="ml-0.5 text-danger">
              *
            </span>
          ) : null}
        </label>
      ) : null}
      {description ? (
        <p id={ids.description} className="text-caption text-fg-muted">
          {description}
        </p>
      ) : null}
      {children}
      {help ? (
        <p id={ids.help} className="text-caption text-fg-muted">
          {help}
        </p>
      ) : null}
      {hasErrors ? (
        <p id={ids.error} role="alert" className="text-caption font-medium text-danger">
          {errors!.join(", ")}
        </p>
      ) : null}
    </div>
  );
}
