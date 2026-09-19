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
import { useTranslation } from "react-i18next";
import { Button, CONTROL, Field, Icon, Select, Textarea } from "../ui";

/** What the form renders beside its submit: a cancel, a secondary action. */
export const FormActionsContext = createContext<ReactNode>(null);
/** What a caller puts under the last field and above the submit line (a panel, a preview). */
export const FormAfterFieldsContext = createContext<ReactNode>(null);

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
  const input = <Input {...props} hasErrors={hasErrors} isRange={isRange} />;
  // The placeholder is an example the field accepts (Architecture/09 §2, T-1604), so the person
  // may take it instead of retyping it. Offered only while the field is empty: filling a field
  // that already holds something is an edit nobody asked for.
  const example = typeof props.placeholder === "string" ? props.placeholder.trim() : "";
  const empty = props.value === undefined || props.value === null || props.value === "";
  if (example === "" || props.readonly || props.disabled || isRange) {
    return input;
  }
  const numeric = props.schema.type === "number" || props.schema.type === "integer";
  return (
    <div className="flex items-start gap-1.5">
      <div className="min-w-0 flex-1">{input}</div>
      {/*
        The action stays in the page for as long as the field has an example, and steps out of
        sight and out of the accessibility tree once the field holds something. Unmounting it
        instead moved the input into a different parent on the first keystroke, React built a new
        one, and the person lost the caret after one character — measured on the endpoint form on
        2026-09-19 (T-2251).
      */}
      <UseExample
        available={empty}
        onUse={() => props.onChange(numeric ? Number(example) : example)}
      />
    </div>
  );
}

/** The one action that writes a field's example into it (T-1604). */
function UseExample({
  onUse,
  available,
}: {
  onUse: () => void;
  available: boolean;
}): React.JSX.Element {
  const { t } = useTranslation();
  return (
    <Button
      type="button"
      variant="secondary"
      size="sm"
      onClick={onUse}
      disabled={!available}
      aria-hidden={available ? undefined : true}
      tabIndex={available ? undefined : -1}
      className={clsx("shrink-0", !available && "invisible")}
    >
      {t("form.useExample")}
    </Button>
  );
}

function Input(
  props: WidgetProps & { hasErrors: boolean; isRange: boolean },
): React.JSX.Element {
  const { hasErrors, isRange, ...rest } = props;
  const forwarded = rest as WidgetProps;
  return (
    <DefaultBaseInput
      {...forwarded}
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
  const { id, label, children, rawErrors, rawDescription, description, rawHelp, displayLabel, required, hidden, schema } =
    props;

  if (hidden) {
    return <div style={{ display: "none" }}>{children}</div>;
  }

  const showLabel = displayLabel !== false && Boolean(label);
  // An array or an object template writes its own heading and description; once is enough.
  const ownHeading = schema.type === "array" || schema.type === "object";
  // What a person reads under the label is written for them: the UiSchema's `help`, in their own
  // language. The schema's description is the field's rustdoc — carried into the API and MCP for
  // engineers — so it is the fallback, never the first thing shown (T-1604, Architecture/09 §2).
  const forPeople = typeof rawHelp === "string" && rawHelp.trim() !== "" ? rawHelp : undefined;
  const descText = ownHeading ? undefined : forPeople ?? description ?? rawDescription;

  return (
    <Field
      id={id}
      label={label}
      hideLabel={!showLabel}
      required={required}
      description={descText}
      help={forPeople === undefined || ownHeading ? rawHelp : undefined}
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
  /** Shut when the form opens; the title and the description are the handle (T-1607). */
  folded?: boolean;
  fields: string[];
}

/** The groups a UiSchema manifest put on this object, if it put any. */
function groupsOf(uiSchema: ObjectFieldTemplateProps["uiSchema"]): RenderedGroup[] {
  const options = uiSchema?.["ui:options"] as { groups?: RenderedGroup[] } | undefined;
  return Array.isArray(options?.groups) ? options.groups : [];
}

/** The fields of one object laid out in cells: two columns on a wide screen, one on a phone. */
function Cells({
  properties,
  schema,
  uiSchema,
}: {
  properties: ObjectFieldTemplateProps["properties"];
  schema: ObjectFieldTemplateProps["schema"];
  uiSchema: ObjectFieldTemplateProps["uiSchema"];
}): React.JSX.Element {
  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
      {properties.map((prop) => (
        <div
          key={prop.name}
          className={clsx("min-w-0", !scalar(schema, uiSchema, prop.name) && "md:col-span-2")}
        >
          {prop.content}
        </div>
      ))}
    </div>
  );
}

/** A one-line field (text, number, a switch, a select) shares its row; lists, objects and text areas do not. */
function scalar(
  schema: ObjectFieldTemplateProps["schema"],
  uiSchema: ObjectFieldTemplateProps["uiSchema"],
  name: string,
): boolean {
  const property = schema.properties?.[name];
  if (typeof property !== "object" || property === null) return false;
  const type = (property as { type?: unknown }).type;
  const widget = (uiSchema?.[name] as { "ui:widget"?: unknown } | undefined)?.["ui:widget"];
  return (
    (type === "string" || type === "number" || type === "integer" || type === "boolean") &&
    widget !== "textarea" &&
    widget !== "checkboxes"
  );
}

function Fieldset({
  title,
  description,
  folded,
  children,
}: {
  title?: ReactNode;
  description?: ReactNode;
  /** Folded shut until the person opens it: `<details>` does the work, keyboard included. */
  folded?: boolean;
  children: ReactNode;
}): React.JSX.Element {
  const box = "flex flex-col gap-4 rounded-lg border border-border bg-surface-subtle/60 p-4";
  if (folded && title) {
    return (
      <details className={box}>
        <summary className="cursor-pointer text-body font-semibold text-fg marker:text-fg-muted">
          {title}
          {description ? (
            <span className="ml-2 font-normal text-caption text-fg-muted">{description}</span>
          ) : null}
        </summary>
        {children}
      </details>
    );
  }
  return (
    <fieldset className={box}>
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
          <Fieldset
            key={group.title ?? at}
            title={group.title}
            description={group.description}
            folded={group.folded}
          >
            <Cells
              properties={properties.filter((prop) => group.fields.includes(prop.name))}
              schema={props.schema}
              uiSchema={uiSchema}
            />
          </Fieldset>
        ))}
        <Cells
          properties={properties.filter((prop) => !grouped.has(prop.name))}
          schema={props.schema}
          uiSchema={uiSchema}
        />
      </div>
    );
  }

  // An object with no title of its own (a title map asking for one language) reads as its
  // fields, not as a boxed section around them.
  if (!title) return <>{properties.map((prop) => prop.content)}</>;
  return (
    <Fieldset title={title} description={description}>
      <Cells properties={properties} schema={props.schema} uiSchema={uiSchema} />
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
  const after = useContext(FormAfterFieldsContext);
  // rjsf hands the button its options under `ui:options`, never the raw key the caller wrote.
  const options = getSubmitButtonOptions(props.uiSchema);
  if (options.norender) {
    return null;
  }
  // A gate the caller closes (PL-49, UI-47): the button stays visible, disabled, with the reason
  // beside it, never hidden.
  const gate = (options.props ?? {}) as { disabled?: boolean; title?: string; loading?: boolean };
  return (
    <>
      {after ? <div className="mt-4 flex flex-col gap-3">{after}</div> : null}
      <div className="mt-6 flex flex-wrap items-center justify-end gap-2 border-t border-border pt-4">
        {gate.disabled && gate.title ? (
          <span role="status" className="text-caption text-fg-muted">
            {gate.title}
          </span>
        ) : null}
        {secondary}
        <Button
          type="submit"
          variant="primary"
          disabled={gate.disabled}
          title={gate.title}
          loading={gate.loading}
        >
          {options.submitText}
        </Button>
      </div>
    </>
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
