import React from "react";
import type { ReactNode } from "react";
import Form from "@rjsf/core";
import validator from "@rjsf/validator-ajv8";
import type { RJSFValidationError } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import type { JsonSchema, UiSchema } from "./types";
import { FormActionsContext, portalTemplates, portalThemeWidgets } from "./theme";

export interface SchemaFormProps<T> {
  schema: JsonSchema;
  uiSchema?: UiSchema;
  formData?: T;
  disabled?: boolean;
  submitLabel?: string;
  /** Rendered beside the submit, on its left: a cancel, a secondary action. */
  actions?: ReactNode;
  onSubmit: (data: T) => void;
  onChange?: (data: T | undefined) => void;
}

const ajvErrorKeyMap: Record<string, string> = {
  required: "form.required",
  type: "form.type",
  minLength: "form.minLength",
  maxLength: "form.maxLength",
  pattern: "form.pattern",
  enum: "form.enum",
  format: "form.format",
};

/**
 * Every schema-driven form of the Portal: rjsf with the Portal's templates and widgets, live
 * validation with translated messages, and no error list (each field carries its own).
 */
export function SchemaForm<T>(props: SchemaFormProps<T>): React.JSX.Element {
  const { schema, uiSchema, formData, disabled, submitLabel, actions, onSubmit, onChange } = props;
  const { t } = useTranslation();

  const effectiveUiSchema = React.useMemo(
    () => ({
      ...uiSchema,
      "ui:submitButtonOptions": {
        ...(uiSchema?.["ui:submitButtonOptions"] as Record<string, unknown> | undefined),
        // rjsf's own default is the untranslated word "Submit".
        submitText: submitLabel ?? t("form.submit"),
      },
    }),
    [uiSchema, submitLabel, t],
  );

  const transformErrors = React.useCallback(
    (errors: RJSFValidationError[]): RJSFValidationError[] => {
      return errors.map((error) => {
        const key =
          error.name && ajvErrorKeyMap[error.name] ? ajvErrorKeyMap[error.name] : "form.invalid";
        return {
          ...error,
          message: t(key),
        };
      });
    },
    [t],
  );

  return (
    <FormActionsContext.Provider value={actions ?? null}>
      <Form<T>
        validator={validator}
        schema={schema}
        uiSchema={effectiveUiSchema}
        formData={formData}
        disabled={disabled}
        liveValidate
        showErrorList={false}
        noHtml5Validate
        transformErrors={transformErrors}
        templates={portalTemplates}
        widgets={portalThemeWidgets}
        onSubmit={(data) => {
          onSubmit(data.formData as T);
        }}
        onChange={(data) => {
          onChange?.(data.formData as T | undefined);
        }}
      />
    </FormActionsContext.Provider>
  );
}
