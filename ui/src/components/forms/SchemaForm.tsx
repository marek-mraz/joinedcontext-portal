import React from "react";
import type { ReactNode } from "react";
import Form from "@rjsf/core";
import validator from "./validator";
import type { RJSFValidationError } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import type { JsonSchema, UiSchema } from "./types";
import { FormActionsContext, FormAfterFieldsContext, portalTemplates, portalThemeWidgets } from "./theme";
import { portalWidgets } from "./widgets";
import { DNS1123, ENTITY_TYPE_PATTERN } from "../../schemas/kinds";

/** The theme's widgets and the Portal's own (`secretRef`, `entityPicker`), which a uiSchema names. */
const widgets = { ...portalThemeWidgets, ...portalWidgets };

export interface SchemaFormProps<T> {
  schema: JsonSchema;
  uiSchema?: UiSchema;
  formData?: T;
  disabled?: boolean;
  submitLabel?: string;
  /** Why the submit is closed right now (PL-49): disables the button and says so beside it. */
  submitDisabledReason?: string;
  /** Rendered beside the submit, on its left: a cancel, a secondary action. */
  actions?: ReactNode;
  /** Rendered under the last field, above the submit line. */
  afterFields?: ReactNode;
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
  not: "form.exclusive",
  dependencies: "form.dependency",
};

/**
 * The translation key of one validation error, by the keyword ajv reports it under.
 *
 * A pattern is the one keyword whose generic message says nothing: "does not match pattern"
 * leaves a person with a regular expression to read. The two patterns the platform's own names
 * are made of say what they want instead (T-0960, PF-09), keyed on the pattern rather than on
 * the field, because the same rule governs `name`, a reference and an entity type.
 */
export function errorMessageKey(error: RJSFValidationError, schema?: JsonSchema): string {
  if (error.name === "pattern") {
    switch (patternOf(error, schema)) {
      case DNS1123:
        return "form.dns1123";
      case ENTITY_TYPE_PATTERN:
        return "form.entityType";
      default:
        break;
    }
  }
  return error.name && ajvErrorKeyMap[error.name] ? ajvErrorKeyMap[error.name] : "form.invalid";
}

/**
 * The pattern one error broke, read out of the schema by the path the validator reports.
 *
 * The validator hands over an empty `params`, so the regular expression itself is not on the
 * error; `schemaPath` is (`#/properties/name/pattern`). A path that leads through a `$ref` or
 * anywhere else this walk cannot follow answers `undefined`, and the generic message stands.
 */
function patternOf(error: RJSFValidationError, schema?: JsonSchema): string | undefined {
  if (!schema || typeof error.schemaPath !== "string") {
    return undefined;
  }
  let node: unknown = schema;
  for (const raw of error.schemaPath.split("/").slice(1)) {
    const segment = raw.replace(/~1/g, "/").replace(/~0/g, "~");
    if (typeof node !== "object" || node === null) {
      return undefined;
    }
    node = (node as Record<string, unknown>)[segment];
  }
  return typeof node === "string" ? node : undefined;
}

/**
 * Every schema-driven form of the Portal: rjsf with the Portal's templates and widgets, live
 * validation with translated messages, and no error list (each field carries its own).
 */
export function SchemaForm<T>(props: SchemaFormProps<T>): React.JSX.Element {
  const { schema, uiSchema, formData, disabled, submitLabel, submitDisabledReason, actions, afterFields, onSubmit, onChange } =
    props;
  const { t } = useTranslation();

  const effectiveUiSchema = React.useMemo(
    () => ({
      ...uiSchema,
      "ui:submitButtonOptions": {
        ...(uiSchema?.["ui:submitButtonOptions"] as Record<string, unknown> | undefined),
        // rjsf's own default is the untranslated word "Submit".
        submitText: submitLabel ?? t("form.submit"),
        ...(submitDisabledReason ? { props: { disabled: true, title: submitDisabledReason } } : {}),
      },
    }),
    [uiSchema, submitLabel, submitDisabledReason, t],
  );

  const transformErrors = React.useCallback(
    (errors: RJSFValidationError[]): RJSFValidationError[] => {
      return errors.map((error) => ({ ...error, message: t(errorMessageKey(error, schema)) }));
    },
    [t, schema],
  );

  return (
    <FormActionsContext.Provider value={actions ?? null}>
      <FormAfterFieldsContext.Provider value={afterFields ?? null}>
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
          widgets={widgets}
          onSubmit={(data) => {
            onSubmit(data.formData as T);
          }}
          onChange={(data) => {
            onChange?.(data.formData as T | undefined);
          }}
        />
      </FormAfterFieldsContext.Provider>
    </FormActionsContext.Provider>
  );
}
