import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "../../api/schema";
import { Badge, Table, TableBody, TableCell, TableHead, TableHeaderCell } from "../ui";

export type FieldChange = components["schemas"]["FieldChange"];

export interface PlanDiffViewerProps {
  fields?: FieldChange[] | null;
  className?: string;
}

/** `spec.enabledRepresentations` → "Enabled representations": the last segment, in words. */
function humanize(path: string): string {
  const last = path.replace(/\[\d+\]/g, "").split(".").filter((part) => !/^\d+$/.test(part)).pop() ?? path;
  const words = last.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[-_]/g, " ").toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * A value as a person reads it (T-1385, UI-24): text as text, a list of plain values joined,
 * and anything nested as its JSON folded away, for whoever wants the detail.
 */
function DiffValue({ value }: { value: unknown }): JSX.Element {
  const { t } = useTranslation();

  if (value === undefined || value === null || value === "") {
    return <span className="text-surface-fg/40">—</span>;
  }

  if (value === "[REDACTED]") {
    return (
      <Badge mono tone="neutral">
        {t("form.redacted")}
      </Badge>
    );
  }

  const plain = (item: unknown) => ["string", "number", "boolean"].includes(typeof item);
  if (plain(value)) {
    return <span className="text-sm break-words text-surface-fg">{String(value)}</span>;
  }
  if (Array.isArray(value) && value.every(plain)) {
    return <span className="text-sm break-words text-surface-fg">{value.map(String).join(", ")}</span>;
  }

  const count = Array.isArray(value) ? value.length : Object.keys(value as object).length;
  return (
    <details className="text-sm text-surface-fg">
      <summary className="cursor-pointer">
        {t(Array.isArray(value) ? "diff.items" : "diff.fields", { count })}
      </summary>
      <pre className="mt-1 font-mono text-xs whitespace-pre">{JSON.stringify(value, null, 2)}</pre>
    </details>
  );
}

export function PlanDiffViewer({ fields, className }: PlanDiffViewerProps): JSX.Element {
  const { t } = useTranslation();

  if (!fields || fields.length === 0) {
    return <p className="text-sm text-surface-fg/70">{t("approvals.noChanges")}</p>;
  }

  return (
    <Table caption={t("approvals.diffType")} zebra={false} className={className}>
      <TableHead>
        <TableHeaderCell align="center" className="w-10 px-3 py-2">
          <span className="sr-only">{t("approvals.diffType")}</span>
        </TableHeaderCell>
        <TableHeaderCell className="px-3 py-2">{t("approvals.field")}</TableHeaderCell>
        <TableHeaderCell className="px-3 py-2">{t("approvals.before")}</TableHeaderCell>
        <TableHeaderCell className="px-3 py-2">{t("approvals.after")}</TableHeaderCell>
      </TableHead>
      <TableBody>
        {fields.map((field, idx) => {
          const hasFrom = field.from !== undefined;
          const hasTo = field.to !== undefined;

          let diffType: "added" | "removed" | "changed" = "changed";
          let marker = "~";
          let diffLabel = t("approvals.diffChanged");
          let rowBg = "hover:bg-surface-subtle/50";

          if (!hasFrom && hasTo) {
            diffType = "added";
            marker = "+";
            diffLabel = t("approvals.diffAdded");
            rowBg = "bg-emerald-500/5 hover:bg-emerald-500/10";
          } else if (hasFrom && !hasTo) {
            diffType = "removed";
            marker = "-";
            diffLabel = t("approvals.diffRemoved");
            rowBg = "bg-danger/5 hover:bg-danger/10";
          } else {
            diffType = "changed";
            marker = "~";
            diffLabel = t("approvals.diffChanged");
            rowBg = "bg-amber-500/5 hover:bg-amber-500/10";
          }

          return (
            <tr key={`${field.path}-${idx}`} title={diffLabel} className={rowBg}>
              <TableCell align="center" className="w-10 px-3 py-2 font-mono font-bold select-none text-surface-fg">
                <span aria-hidden="true">{marker}</span>
                <span className="sr-only">{diffLabel}</span>
              </TableCell>
              <TableCell className="px-3 py-2 font-mono text-xs font-medium text-surface-fg align-top">
                {/* A name a person reads, and the path underneath for whoever edits the YAML. */}
                <span className="block font-sans text-sm">
                  {t(`diff.path.${field.path}`, { defaultValue: humanize(field.path) })}
                </span>
                <span className="block text-caption text-fg-subtle">{field.path}</span>
              </TableCell>
              <TableCell className="px-3 py-2 align-top">
                <div className="max-w-xs overflow-x-auto md:max-w-sm lg:max-w-md">
                  {diffType === "added" ? (
                    <span className="text-surface-fg/40">—</span>
                  ) : (
                    <DiffValue value={field.from} />
                  )}
                </div>
              </TableCell>
              <TableCell className="px-3 py-2 align-top">
                <div className="max-w-xs overflow-x-auto md:max-w-sm lg:max-w-md">
                  {diffType === "removed" ? (
                    <span className="text-surface-fg/40">—</span>
                  ) : (
                    <DiffValue value={field.to} />
                  )}
                </div>
              </TableCell>
            </tr>
          );
        })}
      </TableBody>
    </Table>
  );
}
