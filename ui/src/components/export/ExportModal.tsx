import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import * as Dialog from "@radix-ui/react-dialog";
import { api, ApiError, unwrap } from "../../api/client";
import { Button, buttonClass, Select } from "../ui";

export type ExportFormat = "yaml" | "json" | "zip";

export interface ExportTarget {
  /** Plural of a single kind, or absent for the whole project. */
  plural?: string;
  /** One resource name, or absent for every resource of the selection. */
  name?: string;
}

/**
 * The download URL of one export. Built here rather than in the page so the modal, the tests and
 * any future caller agree on it; it is a plain `GET`, which is what makes the download a link the
 * browser can follow instead of a fetch the SPA has to buffer.
 */
export function exportUrl(
  project: string,
  format: ExportFormat,
  target: ExportTarget,
  revision?: string,
): string {
  const query = new URLSearchParams({ format });
  if (target.plural) {
    query.set("kinds", target.plural);
  }
  if (target.name) {
    query.set("names", target.name);
  }
  if (revision) {
    query.set("revision", revision);
  }
  return `/api/v1/projects/${encodeURIComponent(project)}/export?${query.toString()}`;
}

/**
 * One-click download of a manifest, a bundle or the whole project archive, at the current
 * revision or an older one (MF-16, CC-49). No Git knowledge is asked of anyone: the revision
 * picker is the history rendered as sentences.
 *
 * From the project, the first and default choice is the whole project: the archive with every
 * manifest, the schema of every kind and data model, and a README saying what each file means
 * (MF-41). The plain YAML and JSON forms stay one click further, under other formats.
 */
export function ExportModal({
  project,
  target,
  open,
  onOpenChange,
}: {
  project: string;
  target: ExportTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [format, setFormat] = useState<ExportFormat>(target.name ? "yaml" : "zip");
  const [revision, setRevision] = useState<string>("");

  const revisions = useQuery({
    queryKey: ["revisions", project],
    enabled: open,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/revisions", {
          params: { path: { project }, query: { limit: 20 } },
        }),
      ),
  });

  const whole = !target.name && !target.plural;
  const formats: ExportFormat[] = ["yaml", "json", "zip"];

  const option = (value: ExportFormat, label: string, help: string) => (
    <label key={value} className="flex items-start gap-2 text-sm">
      <input
        type="radio"
        name="export-format"
        value={value}
        checked={format === value}
        onChange={() => setFormat(value)}
        className="mt-1"
      />
      <span>
        <span className="font-medium">{label}</span>
        <span className="block text-xs text-surface-fg/60">{help}</span>
      </span>
    </label>
  );

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 max-h-[85vh] w-[min(48rem,94vw)] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded border border-border bg-surface p-6 text-surface-fg shadow-lg">
          <Dialog.Title className="text-lg font-bold">{t("export.title")}</Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-surface-fg/70">
            {target.name
              ? t("export.hintResource", { name: target.name })
              : t("export.hintProject", { project })}
          </Dialog.Description>

          <fieldset className="mt-4">
            <legend className="text-sm font-medium">{t("export.format")}</legend>
            {whole ? (
              <div className="mt-2 space-y-2">
                {option("zip", t("export.formats.whole"), t("export.formats.wholeHelp"))}
                <details className="text-sm">
                  <summary className="cursor-pointer text-xs text-surface-fg/70">
                    {t("export.otherFormats")}
                  </summary>
                  <div className="mt-2 space-y-2">
                    {(["yaml", "json"] as const).map((value) =>
                      option(value, t(`export.formats.${value}`), t(`export.formats.${value}Help`)),
                    )}
                  </div>
                </details>
              </div>
            ) : (
              <div className="mt-2 space-y-2">
                {formats.map((value) =>
                  option(value, t(`export.formats.${value}`), t(`export.formats.${value}Help`)),
                )}
              </div>
            )}
          </fieldset>

          <label className="mt-4 block text-sm font-medium">
            {t("export.revision")}
            <Select
              value={revision}
              onChange={(event) => setRevision(event.target.value)}
              className="mt-1"
            >
              <option value="">{t("export.currentRevision")}</option>
              {(revisions.data?.items ?? []).map((commit) => (
                <option key={commit.sha} value={commit.sha}>
                  {`${commit.sha.slice(0, 7)} · ${commit.message} · ${
                    commit.date ? new Date(commit.date).toLocaleDateString(locale) : ""
                  }`}
                </option>
              ))}
            </Select>
          </label>
          {revisions.isError ? (
            <p role="status" className="mt-1 text-xs text-surface-fg/60">
              {revisions.error instanceof ApiError && revisions.error.status === 503
                ? t("export.noForge")
                : t("export.noHistory")}
            </p>
          ) : null}

          <p className="mt-4 text-xs text-surface-fg/60">{t("export.secretsNote")}</p>

          <div className="mt-6 flex flex-wrap items-center justify-end gap-2">
            <Dialog.Close asChild>
              <Button variant="secondary" size="sm">
                {t("form.cancel")}
              </Button>
            </Dialog.Close>
            <a
              href={exportUrl(project, format, target, revision || undefined)}
              download
              onClick={() => onOpenChange(false)}
              className={buttonClass("primary", "md")}
            >
              {t("export.download")}
            </a>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
