import { Suspense, lazy, useState } from "react";
import type { JSX, ReactNode } from "react";
import validator from "./forms/validator";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { errorMessageKey, SchemaForm } from "./forms/SchemaForm";
import type { JsonSchema, UiSchema } from "./forms/types";
import { Alert, Button, Dialog, DialogClose } from "./ui";
import type { DialogSize } from "./ui";

// Monaco is loaded when the YAML view is opened and not before: it is the heaviest thing in
// the Portal and a dialog that never shows its manifest would otherwise pay for it.
const MonacoSourceView = lazy(() => import("../pages/models/MonacoSourceView"));

/**
 * How the form's data and the manifest it writes translate into each other, for the YAML
 * view of a dialog: what the form submits is the manifest the YAML shows, and the YAML the
 * author edits becomes the form again on the way back.
 */
export interface ManifestSource<T> {
  toManifest: (form: T) => unknown;
  fromManifest: (manifest: unknown) => T;
}

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
  size?: DialogSize;
  /** Adds the YAML view of the manifest beside the form; both edit the same data. */
  source?: ManifestSource<T>;
  /** Rendered above the form: the slug generator, a quota warning, whatever the kind needs. */
  children?: ReactNode;
  onSubmit: (data: T) => void;
  onChange?: (data: T | undefined) => void;
}

type View = "form" | "yaml";

const VIEWS: View[] = ["form", "yaml"];

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
  size,
  source,
  children,
  onSubmit,
  onChange,
}: ResourceFormDialogProps<T>): JSX.Element {
  const { t } = useTranslation();
  const [view, setView] = useState<View>("form");
  const [text, setText] = useState("");
  // A YAML that does not parse, or a manifest the schema refuses: either one keeps the author
  // in the YAML view, where the problem is, rather than losing what they typed.
  const [yamlError, setYamlError] = useState<string | null>(null);
  const [issues, setIssues] = useState<string[]>([]);

  /** A dialog opens on its form again, whatever view it was closed from. */
  function handleOpenChange(next: boolean) {
    if (!next) {
      setView("form");
      setYamlError(null);
      setIssues([]);
    }
    onOpenChange(next);
  }

  /** The form the YAML describes, or `null` with the reason on screen. */
  function readYaml(): T | null {
    if (!source) {
      return null;
    }
    try {
      const document: unknown = parseYaml(text);
      if (!document || typeof document !== "object") {
        throw new Error(t("form.yamlNotManifest"));
      }
      setYamlError(null);
      return source.fromManifest(document);
    } catch (err) {
      setYamlError(t("form.yamlError", { message: err instanceof Error ? err.message : String(err) }));
      return null;
    }
  }

  function show(next: View) {
    if (next === view || !source) {
      return;
    }
    if (next === "yaml") {
      setText(stringifyYaml(source.toManifest(formData ?? ({} as T))));
      setYamlError(null);
      setIssues([]);
      setView("yaml");
      return;
    }
    const form = readYaml();
    if (form === null) {
      return;
    }
    setIssues([]);
    onChange?.(form);
    setView("form");
  }

  /** The YAML view submits what the form view would: the same schema decides (UI-01). */
  function submitYaml() {
    const form = readYaml();
    if (form === null) {
      return;
    }
    const { errors } = validator.validateFormData(form, schema);
    if (errors.length > 0) {
      setIssues(errors.map((issue) => `${issue.property ?? ""} ${t(errorMessageKey(issue))}`.trim()));
      return;
    }
    setIssues([]);
    onChange?.(form);
    onSubmit(form);
  }

  return (
    <Dialog
      open={open}
      onOpenChange={handleOpenChange}
      title={title}
      description={description}
      size={size}
      closeLabel={t("form.cancel")}
    >
      <div className="flex flex-col gap-4">
        {error ? (
          <Alert role="alert" tone="danger">
            {error}
          </Alert>
        ) : null}

        {source ? (
          <div role="tablist" aria-label={title} className="flex flex-wrap gap-1">
            {VIEWS.map((name) => (
              <Button
                key={name}
                role="tab"
                size="sm"
                aria-selected={view === name}
                variant={view === name ? "secondary" : "ghost"}
                onClick={() => show(name)}
              >
                {t(`form.view.${name}`)}
              </Button>
            ))}
          </div>
        ) : null}

        {view === "form" ? (
          <div role="tabpanel" className="flex flex-col gap-4">
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
        ) : (
          <div role="tabpanel" className="flex flex-col gap-3">
            <p className="text-caption text-fg-muted">{t("form.yamlHint")}</p>
            {yamlError ? (
              <Alert role="alert" tone="danger">
                {yamlError}
              </Alert>
            ) : null}
            {issues.length > 0 ? (
              <Alert role="alert" tone="danger">
                <p>{t("form.schemaErrors")}</p>
                <ul className="mt-1 list-disc pl-5">
                  {issues.map((issue) => (
                    <li key={issue} className="font-mono text-caption">
                      {issue}
                    </li>
                  ))}
                </ul>
              </Alert>
            ) : null}
            <div className="overflow-hidden rounded-md border border-border">
              <Suspense fallback={<p className="p-3 text-body">{t("models.loadingEditor")}</p>}>
                <MonacoSourceView
                  value={text}
                  onChange={(next) => {
                    setText(next);
                    setYamlError(null);
                  }}
                  onMount={() => undefined}
                  height="24rem"
                />
              </Suspense>
            </div>
            <div className="flex flex-wrap items-center justify-end gap-2">
              <DialogClose asChild>
                <Button variant="ghost">{t("form.cancel")}</Button>
              </DialogClose>
              <Button variant="primary" disabled={disabled} onClick={submitYaml}>
                {submitLabel}
              </Button>
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}
