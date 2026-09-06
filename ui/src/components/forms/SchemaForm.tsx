import React from "react";
import Form, { getDefaultRegistry } from "@rjsf/core";
import validator from "@rjsf/validator-ajv8";
import { getSubmitButtonOptions } from "@rjsf/utils";
import type {
  FieldTemplateProps,
  ObjectFieldTemplateProps,
  SubmitButtonProps,
  RJSFValidationError,
  WidgetProps,
} from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import type { JsonSchema, UiSchema } from "./types";

export interface SchemaFormProps<T> {
  schema: JsonSchema;
  uiSchema?: UiSchema;
  formData?: T;
  disabled?: boolean;
  submitLabel?: string;
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

function SubmitButton(props: SubmitButtonProps): React.JSX.Element | null {
  const { t } = useTranslation();
  const options = getSubmitButtonOptions(props.uiSchema);
  if (options.norender) {
    return null;
  }
  const submitText =
    (props.uiSchema?.["ui:submitButtonOptions"] as { submitText?: string } | undefined)?.submitText ??
    t("form.submit");

  return (
    <button
      type="submit"
      className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
    >
      {submitText}
    </button>
  );
}

const DefaultBaseInput = getDefaultRegistry().templates.BaseInputTemplate;

/** The default `<input>` with the portal styling, `aria-invalid` when the field has errors.
 * rjsf already points `aria-describedby` at `${id}__error`, which is the id FieldTemplate uses. */
function BaseInputTemplate(props: WidgetProps): React.JSX.Element {
  const hasErrors = Boolean(props.rawErrors && props.rawErrors.length > 0);
  return (
    <DefaultBaseInput
      {...props}
      className="block w-full rounded border border-border bg-surface px-3 py-1.5 text-base text-surface-fg placeholder:text-surface-fg/50 focus:border-border-focus focus:outline-none focus:ring-2 focus:ring-border-focus"
      aria-invalid={hasErrors ? "true" : undefined}
    />
  );
}

function FieldTemplate(props: FieldTemplateProps): React.JSX.Element {
  const {
    id,
    label,
    children,
    rawErrors,
    rawDescription,
    description,
    displayLabel,
    required,
    hidden,
  } = props;

  if (hidden) {
    return <div style={{ display: "none" }}>{children}</div>;
  }

  const hasErrors = Boolean(rawErrors && rawErrors.length > 0);
  const errorId = `${id}__error`;
  const descId = `${id}__description`;

  const showLabel = displayLabel !== false && Boolean(label);
  const descText = description || rawDescription;

  return (
    <div data-invalid={hasErrors ? "true" : undefined} className="mb-4">
      {showLabel && (
        <label htmlFor={id} className="mb-1 block text-sm font-medium text-surface-fg">
          {label}
          {required && <span aria-hidden="true"> *</span>}
        </label>
      )}
      {children}
      {descText && (
        <p id={descId} className="mt-1 text-sm text-surface-fg/70">
          {descText}
        </p>
      )}
      {hasErrors && (
        <p id={errorId} role="alert" className="mt-1 text-sm text-danger">
          {rawErrors!.join(", ")}
        </p>
      )}
    </div>
  );
}

function ObjectFieldTemplate(props: ObjectFieldTemplateProps): React.JSX.Element {
  const { title, description, properties, fieldPathId } = props;
  const isRoot = fieldPathId.$id === "root";

  if (isRoot) {
    return (
      <div className="space-y-4">
        {title && <h2 className="text-lg font-semibold text-surface-fg">{title}</h2>}
        {description && <p className="mb-2 text-sm text-surface-fg/70">{description}</p>}
        {properties.map((prop) => prop.content)}
      </div>
    );
  }

  return (
    <fieldset className="space-y-4 rounded border border-border p-4">
      {title && <legend className="px-1 text-base font-semibold text-surface-fg">{title}</legend>}
      {description && <p className="mb-2 text-sm text-surface-fg/70">{description}</p>}
      {properties.map((prop) => prop.content)}
    </fieldset>
  );
}

const templates = {
  BaseInputTemplate,
  FieldTemplate,
  ObjectFieldTemplate,
  ButtonTemplates: {
    SubmitButton,
  },
};

export function SchemaForm<T>(props: SchemaFormProps<T>): React.JSX.Element {
  const { schema, uiSchema, formData, disabled, submitLabel, onSubmit, onChange } = props;
  const { t } = useTranslation();

  const effectiveUiSchema = React.useMemo(() => {
    if (!submitLabel) {
      return uiSchema;
    }
    return {
      ...uiSchema,
      "ui:submitButtonOptions": {
        ...(uiSchema?.["ui:submitButtonOptions"] as Record<string, unknown> | undefined),
        submitText: submitLabel,
      },
    };
  }, [uiSchema, submitLabel]);

  const transformErrors = React.useCallback(
    (errors: RJSFValidationError[]): RJSFValidationError[] => {
      return errors.map((error) => {
        const key =
          error.name && ajvErrorKeyMap[error.name]
            ? ajvErrorKeyMap[error.name]
            : "form.invalid";
        return {
          ...error,
          message: t(key),
        };
      });
    },
    [t]
  );

  return (
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
      templates={templates}
      onSubmit={(data) => {
        onSubmit(data.formData as T);
      }}
      onChange={(data) => {
        onChange?.(data.formData as T | undefined);
      }}
    />
  );
}
