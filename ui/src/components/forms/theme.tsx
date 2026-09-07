/**
 * The rjsf templates and widgets of the Portal (UI-01, UI-02).
 *
 * Every schema-driven form renders through the components of `components/ui`, so a form
 * generated from a JSON Schema and a form written by hand are the same form. The behaviour
 * of each widget is rjsf's own: the value encoding of a select, the toggling of a checkbox
 * group, the ids of an array item; only the markup and the classes are the Portal's.
 */
import { createContext, useCallback, useContext } from "react";
import type { ChangeEvent, FocusEvent, ReactNode } from "react";
import { getDefaultRegistry } from "@rjsf/core";
import {
  ariaDescribedByIds,
  enumOptionSelectedValue,
  enumOptionValueDecoder,
  enumOptionValueEncoder,
  enumOptionsDeselectValue,
  enumOptionsIsSelected,
  enumOptionsSelectValue,
  getOptionValueFormat,
  getSubmitButtonOptions,
  getUiOptions,
  optionId,
  schemaRequiresTrueValue,
  TranslatableString,
} from "@rjsf/utils";
import type {
  ArrayFieldItemTemplateProps,
  ArrayFieldTemplateProps,
  FieldTemplateProps,
  IconButtonProps,
  ObjectFieldTemplateProps,
  SubmitButtonProps,
  WidgetProps,
} from "@rjsf/utils";
import { clsx } from "clsx";
import { Button, CONTROL, Field, Icon, Select, Textarea } from "../ui";

/** What the form renders beside its submit: a cancel, a secondary action. */
export const FormActionsContext = createContext<ReactNode>(null);

const CHECKBOX =
  "focus-ring size-4 shrink-0 cursor-pointer rounded-sm border-border-strong accent-[var(--portal-primary)] disabled:cursor-not-allowed disabled:opacity-50";

// ---------------------------------------------------------------------------------------------
// Templates

const DefaultBaseInput = getDefaultRegistry().templates.BaseInputTemplate;

/**
 * rjsf's own `<input>` (type mapping, number steps, empty values, examples) in the Portal's
 * skin, `aria-invalid` when the field has errors. rjsf already points `aria-describedby` at
 * `${id}__error`, which is the id the Field gives its error line.
 */
export function BaseInputTemplate(props: WidgetProps): React.JSX.Element {
  const hasErrors = Boolean(props.rawErrors && props.rawErrors.length > 0);
  const isRange = props.type === "range";
  return (
    <DefaultBaseInput
      {...props}
      className={
        isRange
          ? "focus-ring h-9 w-full cursor-pointer accent-[var(--portal-primary)]"
          : clsx(CONTROL, "h-9 px-3", props.type === "number" && "tabular-nums")
      }
      aria-invalid={hasErrors ? "true" : undefined}
    />
  );
}

export function FieldTemplate(props: FieldTemplateProps): React.JSX.Element {
  const { id, label, children, rawErrors, rawDescription, description, rawHelp, displayLabel, required, hidden } =
    props;

  if (hidden) {
    return <div style={{ display: "none" }}>{children}</div>;
  }

  const showLabel = displayLabel !== false && Boolean(label);
  const descText = description || rawDescription;

  return (
    <Field
      id={id}
      label={label}
      hideLabel={!showLabel}
      required={required}
      description={descText}
      help={rawHelp}
      errors={rawErrors}
    >
      {children}
    </Field>
  );
}

/** One visual grouping of a flat schema, as a UiSchema manifest describes it (UI-02). */
interface RenderedGroup {
  title?: string;
  description?: string;
  fields: string[];
}

/** The groups a UiSchema manifest put on this object, if it put any. */
function groupsOf(uiSchema: ObjectFieldTemplateProps["uiSchema"]): RenderedGroup[] {
  const options = uiSchema?.["ui:options"] as { groups?: RenderedGroup[] } | undefined;
  return Array.isArray(options?.groups) ? options.groups : [];
}

function Fieldset({
  title,
  description,
  children,
}: {
  title?: ReactNode;
  description?: ReactNode;
  children: ReactNode;
}): React.JSX.Element {
  return (
    <fieldset className="flex flex-col gap-4 rounded-lg border border-border bg-surface-subtle/60 p-4">
      {title ? <legend className="px-1.5 text-body font-semibold text-fg">{title}</legend> : null}
      {description ? <p className="-mt-1 text-caption text-fg-muted">{description}</p> : null}
      {children}
    </fieldset>
  );
}

export function ObjectFieldTemplate(props: ObjectFieldTemplateProps): React.JSX.Element {
  const { title, description, properties, fieldPathId, uiSchema } = props;
  const isRoot = fieldPathId.$id === "root";

  if (isRoot) {
    // A flat schema has no nested objects to become fieldsets, so the grouping a UiSchema
    // manifest declares is what gives the form its sections. Every property is rendered
    // exactly once: what no group names follows the groups, in the order rjsf ordered it.
    const groups = groupsOf(uiSchema);
    const grouped = new Set(groups.flatMap((group) => group.fields));
    return (
      <div className="flex flex-col gap-4">
        {title ? <h2 className="text-title font-semibold text-fg">{title}</h2> : null}
        {description ? <p className="text-body text-fg-muted">{description}</p> : null}
        {groups.map((group, at) => (
          <Fieldset key={group.title ?? at} title={group.title} description={group.description}>
            {properties
              .filter((prop) => group.fields.includes(prop.name))
              .map((prop) => prop.content)}
          </Fieldset>
        ))}
        {properties.filter((prop) => !grouped.has(prop.name)).map((prop) => prop.content)}
      </div>
    );
  }

  return (
    <Fieldset title={title} description={description}>
      {properties.map((prop) => prop.content)}
    </Fieldset>
  );
}

export function ArrayFieldTemplate(props: ArrayFieldTemplateProps): React.JSX.Element {
  const { canAdd, disabled, fieldPathId, uiSchema, items, onAddClick, readonly, registry, required, schema, title } =
    props;
  const uiOptions = getUiOptions(uiSchema);
  const {
    ButtonTemplates: { AddButton },
  } = registry.templates;
  const heading = (uiOptions.title as string | undefined) || title;
  const description = (uiOptions.description as string | undefined) || schema.description;
  return (
    <fieldset id={fieldPathId.$id} className="flex flex-col gap-2">
      {heading ? (
        <legend className="text-body font-medium text-fg">
          {heading}
          {required ? (
            <span aria-hidden="true" className="ml-0.5 text-danger">
              *
            </span>
          ) : null}
        </legend>
      ) : null}
      {description ? <p className="text-caption text-fg-muted">{description}</p> : null}
      {items.length > 0 ? <div className="flex flex-col gap-2">{items}</div> : null}
      {canAdd ? (
        <div>
          <AddButton
            id={`${fieldPathId.$id}__add`}
            className="rjsf-array-item-add"
            onClick={onAddClick}
            disabled={disabled || readonly}
            uiSchema={uiSchema}
            registry={registry}
          />
        </div>
      ) : null}
    </fieldset>
  );
}

export function ArrayFieldItemTemplate(props: ArrayFieldItemTemplateProps): React.JSX.Element {
  const { children, buttonsProps, hasToolbar, registry, uiSchema } = props;
  const { ArrayFieldItemButtonsTemplate } = registry.templates;
  return (
    <div className="flex items-start gap-2 rounded-md border border-border bg-surface p-2">
      <div className="min-w-0 flex-1 [&>div]:gap-1">{children}</div>
      {hasToolbar ? (
        <div className="flex shrink-0 items-center gap-1">
          <ArrayFieldItemButtonsTemplate {...buttonsProps} uiSchema={uiSchema} registry={registry} />
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Buttons

export function SubmitButton(props: SubmitButtonProps): React.JSX.Element | null {
  const secondary = useContext(FormActionsContext);
  // rjsf hands the button its options under `ui:options`, never the raw key the caller wrote.
  const options = getSubmitButtonOptions(props.uiSchema);
  if (options.norender) {
    return null;
  }
  return (
    <div className="mt-6 flex flex-wrap items-center justify-end gap-2 border-t border-border pt-4">
      {secondary}
      <Button type="submit" variant="primary">
        {options.submitText}
      </Button>
    </div>
  );
}

function iconButton(
  icon: "plus" | "trash" | "arrowUp" | "arrowDown" | "copy",
  string: TranslatableString,
  danger = false,
  withLabel = false,
) {
  return function RjsfIconButton(props: IconButtonProps): React.JSX.Element {
    const { registry, className, id, onClick, disabled } = props;
    const label = registry.translateString(string);
    return (
      <Button
        id={id}
        onClick={onClick}
        disabled={disabled}
        type="button"
        variant={danger ? "ghost" : "secondary"}
        size="sm"
        title={label}
        aria-label={withLabel ? undefined : label}
        className={clsx(danger && "text-danger hover:bg-danger-soft", className)}
        icon={<Icon name={icon} className="size-4" />}
      >
        {withLabel ? label : null}
      </Button>
    );
  };
}

export const AddButton = iconButton("plus", TranslatableString.AddButton, false, true);
export const RemoveButton = iconButton("trash", TranslatableString.RemoveButton, true);
export const MoveUpButton = iconButton("arrowUp", TranslatableString.MoveUpButton);
export const MoveDownButton = iconButton("arrowDown", TranslatableString.MoveDownButton);
export const CopyButton = iconButton("copy", TranslatableString.CopyButton);

// ---------------------------------------------------------------------------------------------
// Widgets

function selectValue(event: ChangeEvent<HTMLSelectElement> | FocusEvent<HTMLSelectElement>, multiple: boolean) {
  if (multiple) {
    return Array.from(event.target.options)
      .filter((option) => option.selected)
      .map((option) => option.value);
  }
  return event.target.value;
}

export function SelectWidget(props: WidgetProps): React.JSX.Element {
  const {
    schema,
    id,
    options,
    value,
    required,
    disabled,
    readonly,
    multiple = false,
    autofocus = false,
    onChange,
    onBlur,
    onFocus,
    placeholder,
    htmlName,
    rawErrors,
  } = props;
  const { enumOptions, enumDisabled, emptyValue: optEmptyVal } = options;
  const emptyValue = multiple ? [] : "";
  const optionValueFormat = getOptionValueFormat(options);
  const hasErrors = Boolean(rawErrors && rawErrors.length > 0);

  const handleFocus = useCallback(
    (event: FocusEvent<HTMLSelectElement>) =>
      onFocus(id, enumOptionValueDecoder(selectValue(event, multiple), enumOptions, optionValueFormat, optEmptyVal)),
    [onFocus, id, multiple, enumOptions, optEmptyVal, optionValueFormat],
  );
  const handleBlur = useCallback(
    (event: FocusEvent<HTMLSelectElement>) =>
      onBlur(id, enumOptionValueDecoder(selectValue(event, multiple), enumOptions, optionValueFormat, optEmptyVal)),
    [onBlur, id, multiple, enumOptions, optEmptyVal, optionValueFormat],
  );
  const handleChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) =>
      onChange(enumOptionValueDecoder(selectValue(event, multiple), enumOptions, optionValueFormat, optEmptyVal)),
    [onChange, multiple, enumOptions, optEmptyVal, optionValueFormat],
  );

  const selected = enumOptionSelectedValue(value, enumOptions, multiple, optionValueFormat, emptyValue);
  const showPlaceholder = !multiple && schema.default === undefined;

  return (
    <Select
      id={id}
      name={htmlName || id}
      multiple={multiple}
      value={selected as string | string[]}
      required={required}
      disabled={disabled || readonly}
      autoFocus={autofocus}
      aria-invalid={hasErrors ? "true" : undefined}
      aria-describedby={ariaDescribedByIds(id)}
      onBlur={handleBlur}
      onFocus={handleFocus}
      onChange={handleChange}
    >
      {showPlaceholder ? <option value="">{placeholder}</option> : null}
      {Array.isArray(enumOptions)
        ? enumOptions.map(({ value: enumValue, label }, index) => (
            <option
              key={String(enumValue)}
              value={enumOptionValueEncoder(enumValue, index, optionValueFormat)}
              disabled={Array.isArray(enumDisabled) && enumDisabled.includes(enumValue)}
            >
              {label}
            </option>
          ))
        : null}
    </Select>
  );
}

export function TextareaWidget(props: WidgetProps): React.JSX.Element {
  const { id, options, placeholder, value, required, disabled, readonly, autofocus = false, onChange, onBlur, onFocus, htmlName, rawErrors } =
    props;
  const hasErrors = Boolean(rawErrors && rawErrors.length > 0);
  return (
    <Textarea
      id={id}
      name={htmlName || id}
      value={typeof value === "string" ? value : (value ?? "")}
      placeholder={placeholder}
      required={required}
      disabled={disabled}
      readOnly={readonly}
      autoFocus={autofocus}
      rows={typeof options.rows === "number" ? options.rows : undefined}
      aria-invalid={hasErrors ? "true" : undefined}
      aria-describedby={ariaDescribedByIds(id)}
      onChange={(event) => onChange(event.target.value === "" ? options.emptyValue : event.target.value)}
      onBlur={(event) => onBlur(id, event.target.value)}
      onFocus={(event) => onFocus(id, event.target.value)}
    />
  );
}

export function CheckboxWidget(props: WidgetProps): React.JSX.Element {
  const { schema, uiSchema, options, id, value, disabled, readonly, label, hideLabel, autofocus = false, onBlur, onFocus, onChange, htmlName } =
    props;
  // Because an unchecked checkbox will cause html5 validation to fail, only add the
  // "required" attribute if the field value must be "true", due to "const" or "enum".
  const required = schemaRequiresTrueValue(schema);
  const uiOptions = getUiOptions(uiSchema);
  const description = uiOptions.widget === "checkbox" ? undefined : (options.description ?? schema.description);
  return (
    <div className="flex flex-col gap-1">
      <label className={clsx("inline-flex cursor-pointer items-center gap-2.5 text-body text-fg", (disabled || readonly) && "cursor-not-allowed opacity-60")}>
        <input
          type="checkbox"
          id={id}
          name={htmlName || id}
          checked={typeof value === "undefined" ? false : Boolean(value)}
          required={required}
          disabled={disabled || readonly}
          autoFocus={autofocus}
          onChange={(event) => onChange(event.target.checked)}
          onBlur={(event) => onBlur(id, event.target.checked)}
          onFocus={(event) => onFocus(id, event.target.checked)}
          aria-describedby={ariaDescribedByIds(id)}
          className={CHECKBOX}
        />
        {hideLabel ? null : <span>{label}</span>}
      </label>
      {!hideLabel && description ? (
        <p id={`${id}__description`} className="pl-6.5 text-caption text-fg-muted">
          {description}
        </p>
      ) : null}
    </div>
  );
}

export function CheckboxesWidget(props: WidgetProps): React.JSX.Element {
  const { id, disabled, options, value, autofocus = false, readonly, onChange, onBlur, onFocus, htmlName } = props;
  const { inline = false, enumOptions, enumDisabled, emptyValue } = options;
  const optionValueFormat = getOptionValueFormat(options);
  const values = Array.isArray(value) ? value : [value];
  const decode = (target: HTMLInputElement) =>
    enumOptionValueDecoder(target.value, enumOptions, optionValueFormat, emptyValue);

  return (
    <div id={id} className={clsx("flex gap-x-5 gap-y-2", inline ? "flex-row flex-wrap" : "flex-col sm:flex-row sm:flex-wrap")}>
      {Array.isArray(enumOptions)
        ? enumOptions.map((option, index) => {
            const checked = enumOptionsIsSelected(option.value, values);
            const itemDisabled = Array.isArray(enumDisabled) && enumDisabled.includes(option.value);
            const off = disabled || itemDisabled || readonly;
            return (
              <label
                key={String(option.value)}
                className={clsx("inline-flex cursor-pointer items-center gap-2 text-body text-fg", off && "cursor-not-allowed opacity-60")}
              >
                <input
                  type="checkbox"
                  id={optionId(id, index)}
                  name={htmlName || id}
                  checked={checked}
                  value={enumOptionValueEncoder(option.value, index, optionValueFormat)}
                  disabled={off}
                  autoFocus={autofocus && index === 0}
                  onChange={(event) =>
                    onChange(
                      event.target.checked
                        ? enumOptionsSelectValue(index, values, enumOptions)
                        : enumOptionsDeselectValue(index, values, enumOptions),
                    )
                  }
                  onBlur={(event) => onBlur(id, decode(event.target))}
                  onFocus={(event) => onFocus(id, decode(event.target))}
                  aria-describedby={ariaDescribedByIds(id)}
                  className={CHECKBOX}
                />
                <span>{option.label}</span>
              </label>
            );
          })
        : null}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------

export const portalTemplates = {
  BaseInputTemplate,
  FieldTemplate,
  ObjectFieldTemplate,
  ArrayFieldTemplate,
  ArrayFieldItemTemplate,
  ButtonTemplates: {
    SubmitButton,
    AddButton,
    RemoveButton,
    MoveUpButton,
    MoveDownButton,
    CopyButton,
  },
};

export const portalThemeWidgets = {
  SelectWidget,
  TextareaWidget,
  CheckboxWidget,
  CheckboxesWidget,
};
