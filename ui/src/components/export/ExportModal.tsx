import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import * as Dialog from "@radix-ui/react-dialog";
import { api, ApiError, unwrap } from "../../api/client";
import { Button, Select } from "../ui";

export type ExportFormat = "yaml" | "json" | "zip";

export interface ExportTarget {
  /** Plural of a single kind, or absent for the whole project. */
  plural?: string;
  /** One resource name, or absent for every resource of the selection. */
  name?: string;
}

/**
 * The download URL of one export. Built here rather than in the page so the modal, the tests and
 * any future caller agree on it.
 *
 * It is fetched rather than followed as a link: a link hands the browser whatever comes back, so a
 * 403 or a 500 was saved as the export and the dialog closed on top of it (MF-16, T-1487).
 * ponytail: the archive is buffered in the browser; stream to disk when an export passes about
 * 100 MB.
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

/** The name the server gave the file, or the one this selection would have. */
function filenameOf(
  answer: Response,
  project: string,
  format: ExportFormat,
  target: ExportTarget,
): string {
  const disposition = answer.headers.get("content-disposition") ?? "";
  const quoted = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(disposition);
  if (quoted?.[1]) {
    return decodeURIComponent(quoted[1]);
  }
  const extension = format === "zip" ? "zip" : format;
  return `${target.name ?? target.plural ?? project}.${extension}`;
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
  const [refused, setRefused] = useState<string | null>(null);
  const [preparing, setPreparing] = useState(false);

  /** Fetch it, check the answer, then save it: a refusal keeps the dialog open with the reason. */
  async function download() {
    setRefused(null);
    setPreparing(true);
    try {
      const url = exportUrl(project, format, target, revision || undefined);
      const answer = await fetch(url, { credentials: "same-origin" });
      if (!answer.ok) {
        let reason = answer.statusText || `HTTP ${answer.status}`;
        try {
          const problem = (await answer.json()) as { detail?: string; title?: string };
          reason = problem.detail ?? problem.title ?? reason;
        } catch {
          // Not a problem document: the status is the whole of what the server said.
        }
        setRefused(reason);
        return;
      }
      const blob = await answer.blob();
      const href = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = href;
      link.download = filenameOf(answer, project, format, target);
      link.click();
      URL.revokeObjectURL(href);
      onOpenChange(false);
    } catch (error) {
      setRefused(error instanceof Error ? error.message : String(error));
    } finally {
      setPreparing(false);
    }
  }

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
          {refused ? (
            <p role="alert" className="mt-2 text-caption text-danger">
              {t("export.refused", { reason: refused })}
            </p>
          ) : null}

          <div className="mt-6 flex flex-wrap items-center justify-end gap-2">
            <Dialog.Close asChild>
              <Button variant="secondary" size="sm">
                {t("form.cancel")}
              </Button>
            </Dialog.Close>
            <Button
              size="md"
              disabled={preparing}
              onClick={() => {
                void download();
              }}
            >
              {preparing ? t("export.preparing") : t("export.download")}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
