import { Suspense, lazy, useEffect, useMemo, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import validator from "./forms/validator";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { errorMessageKey, SchemaForm } from "./forms/SchemaForm";
import type { JsonSchema, UiSchema } from "./forms/types";
import { portalThemeWidgets } from "./forms/theme";
import { arrange, index } from "./forms/uischema";
import { portalWidgets } from "./forms/widgets";
import { api, queryKeys, unwrap } from "../api/client";
import { Alert, Badge, Button, Dialog, DialogClose } from "./ui";
import type { DialogSize } from "./ui";
import { useBranding } from "../branding";
import { digestOf, getDraft, putDraft, subscribeDrafts } from "../api/drafts";
import type { Draft, Verdict } from "../api/drafts";

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
  /**
   * The manifest kind this form edits, e.g. `Endpoint`: what lets a
   * `portal/forms/{kind lowercased}.uischema.yaml` arrange it (UI-02). Without a kind, or for a
   * kind nobody wrote a manifest for, the form renders from `uiSchema` alone.
   */
  kind?: string;
  uiSchema?: UiSchema;
  formData?: T;
  submitLabel: string;
  disabled?: boolean;
  /** Why proposing is closed right now (PL-49): both views disable their button and say so. */
  submitDisabledReason?: string;
  error?: string | null;
  size?: DialogSize;
  /** Adds the YAML view of the manifest beside the form; both edit the same data. */
  source?: ManifestSource<T>;
  /** Rendered above the form: the slug generator, a quota warning, whatever the kind needs. */
  children?: ReactNode;
  /** Rendered under the last field, above the submit line: panels that follow the fields. */
  afterFields?: ReactNode;
  project?: string;
  draftKind?: string;
  draftName?: string;
  verdict?: Verdict | null;
  onVerdictChange?: (verdict: Verdict | null) => void;
  onSubmit: (data: T, draft?: { kind: string; name: string }) => void;
  onChange?: (data: T | undefined) => void;
}

type View = "form" | "yaml";

const VIEWS: View[] = ["form", "yaml"];

function extractName(form: unknown): string | undefined {
  if (!form || typeof form !== "object") return undefined;
  const f = form as Record<string, unknown>;
  if (typeof f.name === "string" && f.name.trim()) return f.name.trim();
  if (f.metadata && typeof f.metadata === "object") {
    const m = f.metadata as Record<string, unknown>;
    if (typeof m.name === "string" && m.name.trim()) return m.name.trim();
  }
  return undefined;
}

function formatAge(isoString?: string): string {
  if (!isoString) return "";
  const ms = Date.now() - new Date(isoString).getTime();
  if (Number.isNaN(ms) || ms < 0) return "just now";
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ago`;
}

/** One modal for every manifest form: the schema decides the fields, the caller the kind. */
export function ResourceFormDialog<T>({
  open,
  onOpenChange,
  title,
  description,
  schema,
  kind,
  uiSchema,
  formData,
  submitLabel,
  disabled,
  submitDisabledReason,
  error,
  size = "xl",
  source,
  children,
  afterFields,
  project,
  draftKind,
  draftName,
  verdict: externalVerdict,
  onVerdictChange,
  onSubmit,
  onChange,
}: ResourceFormDialogProps<T>): JSX.Element {
  const { t, i18n } = useTranslation();
  const branding = useBranding();

  // One fetch for the whole of `portal/forms/`, shared by every dialog through the query cache.
  const forms = useQuery({
    queryKey: queryKeys.forms(),
    enabled: open && kind !== undefined,
    staleTime: 5 * 60_000,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/forms", {})),
  });
  // CC-29: the Git mechanics are on the form only when the person asked for them. The stored
  // preference starts it; the switch answers at once and saves in the background, so an
  // instance without a preferences database still has a working switch.
  const preferences = useQuery({
    queryKey: queryKeys.preferences(),
    enabled: open && kind !== undefined,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/preferences", {})),
  });
  const [advancedChoice, setAdvancedChoice] = useState<boolean | null>(null);
  const advanced = advancedChoice ?? preferences.data?.advancedMode === true;
  const saveAdvanced = useMutation({
    mutationFn: async (next: boolean) =>
      unwrap(await api.PUT("/api/v1/preferences", { body: { ...(preferences.data ?? {}), advancedMode: next } })),
  });

  const arranged = useMemo(() => {
    if (!kind || !forms.data) {
      return undefined;
    }
    const indexed = index(forms.data.items ?? []);
    const manifest = indexed.forms[kind];
    if (!manifest) {
      return { uiSchema: undefined, problems: indexed.problems, advancedFields: false };
    }
    const result = arrange(manifest, {
      locale: i18n.language,
      properties: Object.keys(schema.properties ?? {}),
      widgets: [...Object.keys(portalThemeWidgets), ...Object.keys(portalWidgets)],
      advanced,
    });
    const advancedFields = Object.values(manifest.spec.fields ?? {}).some((field) => field.advanced === true);
    return { uiSchema: result.uiSchema, problems: [...indexed.problems, ...result.problems], advancedFields };
  }, [kind, forms.data, schema, i18n.language, advanced]);

  // The manifest arranges what it names; the caller's literal still covers what a manifest
  // cannot know, such as a name that is read-only once the resource exists.
  const effectiveUiSchema = useMemo<UiSchema | undefined>(() => {
    if (!arranged?.uiSchema) {
      return uiSchema;
    }
    const merged: Record<string, unknown> = { ...(arranged.uiSchema as Record<string, unknown>) };
    for (const [key, value] of Object.entries((uiSchema ?? {}) as Record<string, unknown>)) {
      const own = merged[key];
      merged[key] =
        own && value && typeof own === "object" && typeof value === "object" && !Array.isArray(own) && !Array.isArray(value)
          ? { ...(own as object), ...(value as object) }
          : value;
    }
    return merged as UiSchema;
  }, [arranged, uiSchema]);
  const formProblems = arranged?.problems ?? [];
  const isLax = (branding as { validation?: string })?.validation === "lax";
  const isStrict = !isLax;

  const [view, setView] = useState<View>("form");
  const [text, setText] = useState("");
  const [yamlError, setYamlError] = useState<string | null>(null);
  const [issues, setIssues] = useState<string[]>([]);

  const [currentDraft, setCurrentDraft] = useState<Draft | null>(null);
  // The draft name whose load has answered, with or without a draft: the write below waits
  // for it, and a name that has no draft yet must not wait forever (T-0631).
  const [loadedName, setLoadedName] = useState<string | undefined>(undefined);
  const [ownVerdict, setInternalVerdict] = useState<Verdict | null>(null);
  const internalVerdict =
    externalVerdict !== undefined ? externalVerdict : ownVerdict;

  const currentManifest = useMemo(() => {
    if (!formData) return {};
    return source ? source.toManifest(formData) : formData;
  }, [formData, source]);

  const currentDigest = useMemo(
    () => digestOf(currentManifest),
    [currentManifest],
  );
  const [conflict, setConflict] = useState<string | null>(null);
  const lastVersionRef = useRef<number | undefined>(undefined);
  const lastTypedRef = useRef<number>(0);
  /** The digest of the manifest the draft last held: an unchanged form writes nothing. */
  const syncedDigestRef = useRef<string | undefined>(undefined);

  const activeName = draftName || extractName(formData);

  const updateVerdict = (v: Verdict | null) => {
    setInternalVerdict(v);
    onVerdictChange?.(v);
  };

  /** A dialog opens on its form again, whatever view it was closed from. */
  function handleOpenChange(next: boolean) {
    if (!next) {
      setView("form");
      setYamlError(null);
      setIssues([]);
      setConflict(null);
      setCurrentDraft(null);
      setLoadedName(undefined);
      lastVersionRef.current = undefined;
    }
    onOpenChange(next);
  }

  // Load existing draft on open if draftName is set
  useEffect(() => {
    if (!open || !draftKind || !project || !draftName) {
      return;
    }
    let active = true;
    void getDraft(project, draftKind, draftName).then((d) => {
      if (!active) return;
      setLoadedName(draftName);
      if (!d) return;
      setCurrentDraft(d);
      lastVersionRef.current = d.version;
      if (d.verdict !== undefined) {
        updateVerdict(d.verdict ?? null);
      }
      if (d.manifest) {
        const loaded = source
          ? source.fromManifest(d.manifest)
          : (d.manifest as T);
        syncedDigestRef.current = digestOf(d.manifest);
        onChange?.(loaded);
      }
    });
    return () => {
      active = false;
    };
  }, [open, draftKind, project, draftName]);

  // Debounced write of draft on form change (600 ms)
  useEffect(() => {
    if (!open || !draftKind || !project || !activeName || !formData) {
      return;
    }
    // A named draft is loaded before anything is written back: the form's first, empty
    // data must not overwrite what another window typed.
    if (draftName && loadedName !== draftName) {
      return;
    }
    const manifest = source ? source.toManifest(formData) : formData;
    const digest = digestOf(manifest);
    if (digest === syncedDigestRef.current) {
      return;
    }
    lastTypedRef.current = Date.now();
    const timer = setTimeout(() => {
      // A proposal may have written this manifest already (see `draftRefFor`).
      if (digest === syncedDigestRef.current) {
        return;
      }
      void putDraft(
        project,
        draftKind,
        activeName,
        manifest,
        lastVersionRef.current,
      )
        .then((d) => {
          syncedDigestRef.current = digest;
          setCurrentDraft(d);
          lastVersionRef.current = d.version;
          if (d.verdict !== undefined) {
            updateVerdict(d.verdict ?? null);
          }
          setConflict(null);
        })
        .catch((err: unknown) => {
          const isConflict =
            typeof err === "object" &&
            err !== null &&
            (err as { status?: number }).status === 409;
          if (isConflict) {
            setConflict(t("drafts.conflict"));
            void getDraft(project, draftKind, activeName).then((reloaded) => {
              if (reloaded) {
                setCurrentDraft(reloaded);
                lastVersionRef.current = reloaded.version;
                if (reloaded.verdict !== undefined) {
                  updateVerdict(reloaded.verdict ?? null);
                }
                if (reloaded.manifest) {
                  const loaded = source
                    ? source.fromManifest(reloaded.manifest)
                    : (reloaded.manifest as T);
                  syncedDigestRef.current = digestOf(reloaded.manifest);
                  onChange?.(loaded);
                }
              }
            });
          }
        });
    }, 600);

    return () => {
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- currentDigest stands for the manifest a page builds around the form (a hidden attribute lives outside it)
  }, [formData, currentDigest, open, draftKind, project, activeName, draftName, loadedName]);

  // Subscribe to project draft events
  const hasDraft = currentDraft !== null;
  useEffect(() => {
    if (!open || !draftKind || !project || !hasDraft) {
      return;
    }
    return subscribeDrafts(project, (event) => {
      if (event.kind === draftKind && event.name === activeName) {
        if (event.event === "drop") {
          setCurrentDraft(null);
          updateVerdict(null);
          return;
        }
        if (event.event === "verdict") {
          void getDraft(project, draftKind, activeName).then((d) => {
            if (d) {
              setCurrentDraft(d);
              updateVerdict(d.verdict ?? null);
            }
          });
          return;
        }
        if (event.version > (lastVersionRef.current ?? 0)) {
          if (Date.now() - lastTypedRef.current < 1500) {
            return;
          }
          void getDraft(project, draftKind, activeName).then((reloaded) => {
            if (reloaded) {
              setCurrentDraft(reloaded);
              lastVersionRef.current = reloaded.version;
              if (reloaded.verdict !== undefined) {
                updateVerdict(reloaded.verdict ?? null);
              }
              if (reloaded.manifest) {
                const loaded = source
                  ? source.fromManifest(reloaded.manifest)
                  : (reloaded.manifest as T);
                syncedDigestRef.current = digestOf(reloaded.manifest);
                onChange?.(loaded);
              }
            }
          });
        }
      }
    });
  }, [open, draftKind, project, activeName, hasDraft]);

  const verdictState = useMemo<"none" | "green" | "red" | "stale">(() => {
    if (!internalVerdict) return "none";
    if (internalVerdict.inputDigest !== currentDigest) return "stale";
    return internalVerdict.ok ? "green" : "red";
  }, [internalVerdict, currentDigest]);

  const proposeReason = useMemo<string | undefined>(() => {
    if (!draftKind || !activeName || !currentDraft) return undefined;
    if (verdictState === "none") return t("drafts.proposeReason.none");
    if (verdictState === "red") return t("drafts.proposeReason.red");
    if (verdictState === "stale") return t("drafts.proposeReason.stale");
    return undefined;
  }, [draftKind, activeName, currentDraft, verdictState, t]);

  const effectiveSubmitDisabledReason = isLax
    ? submitDisabledReason
    : submitDisabledReason || proposeReason;

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
      setYamlError(
        t("form.yamlError", {
          message: err instanceof Error ? err.message : String(err),
        }),
      );
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

  /**
   * The draft a proposal names, holding what the form shows: a proposal takes the draft's own
   * manifest, so the save the debounce has not sent yet goes first (T-0769, AG-61).
   */
  async function draftRefFor(form: T): Promise<{ kind: string; name: string } | undefined> {
    const active = draftName || extractName(form);
    if (!draftKind || !active) {
      return undefined;
    }
    const manifest = source ? source.toManifest(form) : form;
    const digest = digestOf(manifest);
    const loaded = !draftName || loadedName === draftName;
    if (project && loaded && digest !== syncedDigestRef.current) {
      const saved = await putDraft(project, draftKind, active, manifest, lastVersionRef.current);
      syncedDigestRef.current = digest;
      lastVersionRef.current = saved.version;
      setCurrentDraft(saved);
      if (saved.verdict !== undefined) {
        updateVerdict(saved.verdict ?? null);
      }
    }
    return { kind: draftKind, name: active };
  }

  function handleSubmit(form: T) {
    draftRefFor(form).then(
      (draftRef) => onSubmit(form, draftRef),
      () => setConflict(t("drafts.conflict")),
    );
  }

  /** The YAML view submits what the form view would: the same schema decides (UI-01). */
  function submitYaml() {
    const form = readYaml();
    if (form === null) {
      return;
    }
    const { errors } = validator.validateFormData(form, schema);
    if (errors.length > 0) {
      setIssues(
        errors.map((issue) =>
          `${issue.property ?? ""} ${t(errorMessageKey(issue))}`.trim(),
        ),
      );
      return;
    }
    setIssues([]);
    onChange?.(form);
    handleSubmit(form);
  }

  const verdictChip = (
    <span
      data-testid="draft-verdict"
      className={`inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-caption font-medium ${
        verdictState === "green"
          ? "bg-success/15 text-success"
          : verdictState === "red"
            ? "bg-danger/15 text-danger"
            : verdictState === "stale"
              ? "bg-warning/15 text-warning"
              : "bg-surface-muted text-fg-muted"
      }`}
    >
      {verdictState === "none" && t("drafts.verdict.none")}
      {verdictState === "green" &&
        t("drafts.verdict.green", {
          age: formatAge(internalVerdict?.checkedAt),
        })}
      {verdictState === "red" &&
        t("drafts.verdict.red", { age: formatAge(internalVerdict?.checkedAt) })}
      {verdictState === "stale" && t("drafts.verdict.stale")}
    </span>
  );

  const reasonElement = proposeReason ? (
    <span
      id="propose-reason"
      data-testid="propose-reason"
      role="status"
      className={`text-caption ${isLax ? "text-warning" : "text-danger"}`}
    >
      {isLax ? t("drafts.laxWarning") : proposeReason}
    </span>
  ) : null;

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
        {conflict ? (
          <Alert role="alert" tone="warning">
            {conflict}
          </Alert>
        ) : null}
        {error ? (
          <Alert role="alert" tone="danger">
            {error}
          </Alert>
        ) : null}

        {draftKind && (currentDraft || isStrict) ? (
          <div className="flex flex-wrap items-center gap-2">
            {currentDraft ? (
              <span
                data-testid="draft-status"
                className="text-caption text-fg-muted"
              >
                {t("drafts.touched", {
                  who: currentDraft.touchedBy,
                  kind: currentDraft.touchedKind,
                  version: currentDraft.version,
                })}
              </span>
            ) : null}
            {isStrict ? (
              <Badge title={t("drafts.strictHelp")}>{t("drafts.strict")}</Badge>
            ) : null}
          </div>
        ) : null}

        {draftKind &&
        internalVerdict?.findings &&
        internalVerdict.findings.length > 0 ? (
          <div
            data-testid="draft-findings"
            className="flex flex-col gap-1 rounded border border-border bg-surface-subtle p-2 text-caption"
          >
            <span className="font-semibold text-fg">
              {t("drafts.findings")}:
            </span>
            <ul className="list-disc pl-4 space-y-0.5">
              {internalVerdict.findings.map((f, idx) => (
                <li
                  key={idx}
                  className={
                    f.level === "error"
                      ? "text-danger"
                      : f.level === "warning"
                        ? "text-warning"
                        : "text-fg-muted"
                  }
                >
                  {f.path ? (
                    <span className="font-mono">{f.path}: </span>
                  ) : null}
                  <span>{f.message}</span>
                </li>
              ))}
            </ul>
          </div>
        ) : null}

        {source ? (
          <div
            role="tablist"
            aria-label={title}
            className="flex flex-wrap gap-1"
          >
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
            {children ? (
              <div className="flex flex-col gap-3">{children}</div>
            ) : null}

            {formProblems.length > 0 ? (
              <Alert role="status" tone="warning">
                <p>{t("form.uischemaProblems")}</p>
                <ul className="mt-1 list-disc pl-5">
                  {formProblems.map((problem) => (
                    <li key={problem}>{problem}</li>
                  ))}
                </ul>
              </Alert>
            ) : null}

            <SchemaForm<T>
              schema={schema}
              uiSchema={effectiveUiSchema}
              formData={formData}
              disabled={disabled}
              submitLabel={submitLabel}
              submitDisabledReason={effectiveSubmitDisabledReason}
              onSubmit={handleSubmit}
              onChange={onChange}
              afterFields={afterFields}
              actions={
                <div className="flex flex-wrap items-center gap-2">
                  <DialogClose asChild>
                    <Button variant="ghost">{t("form.cancel")}</Button>
                  </DialogClose>
                  {draftKind && activeName ? (
                    <>
                      {verdictChip}
                      {reasonElement}
                    </>
                  ) : null}
                  {arranged?.advancedFields ? (
                    <label className="flex items-center gap-2 text-caption text-fg-muted">
                      <input
                        type="checkbox"
                        checked={advanced}
                        onChange={(event) => {
                          const next = event.target.checked;
                          setAdvancedChoice(next);
                          saveAdvanced.mutate(next);
                        }}
                      />
                      {t("form.advancedMode")}
                    </label>
                  ) : null}
                </div>
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
              <Suspense
                fallback={
                  <p className="p-3 text-body">{t("models.loadingEditor")}</p>
                }
              >
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
              {draftKind && activeName ? (
                <>
                  {verdictChip}
                  {reasonElement}
                </>
              ) : null}
              {effectiveSubmitDisabledReason && !proposeReason ? (
                <span role="status" className="text-caption text-fg-muted">
                  {effectiveSubmitDisabledReason}
                </span>
              ) : null}
              <Button
                variant="primary"
                disabled={disabled || Boolean(effectiveSubmitDisabledReason)}
                title={effectiveSubmitDisabledReason}
                aria-describedby={proposeReason ? "propose-reason" : undefined}
                onClick={submitYaml}
              >
                {submitLabel}
              </Button>
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}
