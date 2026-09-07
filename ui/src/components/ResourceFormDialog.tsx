import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { SchemaForm } from "./forms/SchemaForm";
import type { JsonSchema, UiSchema } from "./forms/types";
import { Alert, Button, Dialog, DialogClose } from "./ui";

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
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={description}
      closeLabel={t("form.cancel")}
    >
      <div className="flex flex-col gap-4">
        {error ? (
          <Alert role="alert" tone="danger">
            {error}
          </Alert>
        ) : null}

        {children ? <div className="flex flex-col gap-3">{children}</div> : null}

        <SchemaForm<T>
          schema={schema}
          uiSchema={uiSchema}
          formData={formData}
          disabled={disabled}
          submitLabel={submitLabel}
          onSubmit={onSubmit}
          onChange={onChange}
          actions={
            <DialogClose asChild>
              <Button variant="ghost">{t("form.cancel")}</Button>
            </DialogClose>
          }
        />
      </div>
    </Dialog>
  );
}
