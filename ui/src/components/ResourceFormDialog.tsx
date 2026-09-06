import type { JSX, ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { useTranslation } from "react-i18next";
import { SchemaForm } from "./forms/SchemaForm";
import type { JsonSchema, UiSchema } from "./forms/types";

export interface ResourceFormDialogProps<T> {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  schema: JsonSchema;
  uiSchema?: UiSchema;
  formData?: T;
  submitLabel: string;
  disabled?: boolean;
  error?: string | null;
  /** Rendered above the form: the slug generator, a quota warning, whatever the kind needs. */
  children?: ReactNode;
  onSubmit: (data: T) => void;
  onChange?: (data: T | undefined) => void;
}

/** One modal for every manifest form: the schema decides the fields, the caller the kind. */
export function ResourceFormDialog<T>({
  open,
  onOpenChange,
  title,
  description,
  schema,
  uiSchema,
  formData,
  submitLabel,
  disabled,
  error,
  children,
  onSubmit,
  onChange,
}: ResourceFormDialogProps<T>): JSX.Element {
  const { t } = useTranslation();

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 max-h-[85vh] w-[min(40rem,92vw)] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded border border-border bg-surface p-6 text-surface-fg shadow-lg">
          <Dialog.Title className="text-lg font-bold">{title}</Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-surface-fg/70">
            {description}
          </Dialog.Description>

          {error ? (
            <div
              role="alert"
              className="mt-4 rounded border border-danger bg-danger/10 p-3 text-sm text-danger"
            >
              {error}
            </div>
          ) : null}

          {children ? <div className="mt-4">{children}</div> : null}

          <div className="mt-4">
            <SchemaForm<T>
              schema={schema}
              uiSchema={uiSchema}
              formData={formData}
              disabled={disabled}
              submitLabel={submitLabel}
              onSubmit={onSubmit}
              onChange={onChange}
            />
          </div>

          <Dialog.Close asChild>
            <button
              type="button"
              className="mt-4 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {t("form.cancel")}
            </button>
          </Dialog.Close>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
