import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import type { components } from "../../api/schema";

export type FieldChange = components["schemas"]["FieldChange"];

export interface PlanDiffViewerProps {
  fields?: FieldChange[] | null;
  className?: string;
}

function DiffValue({ value }: { value: unknown }): JSX.Element {
  const { t } = useTranslation();

  if (value === undefined) {
    return <span className="text-surface-fg/40">—</span>;
  }

  if (value === "[REDACTED]") {
    return (
      <span className="inline-flex items-center rounded border border-border bg-surface-subtle px-1.5 py-0.5 text-xs font-mono font-medium text-surface-fg">
        {t("form.redacted")}
      </span>
    );
  }

  const formatted =
    typeof value === "string" ? JSON.stringify(value) : JSON.stringify(value, null, 2);

  return (
    <pre className="font-mono text-xs whitespace-pre text-surface-fg">
      {formatted}
    </pre>
  );
}

export function PlanDiffViewer({ fields, className }: PlanDiffViewerProps): JSX.Element {
  const { t } = useTranslation();

  if (!fields || fields.length === 0) {
    return <p className="text-sm text-surface-fg/70">{t("approvals.noChanges")}</p>;
  }

  return (
    <div className={clsx("w-full overflow-hidden rounded border border-border", className)}>
      <table className="w-full border-collapse text-left text-sm">
        <caption className="sr-only">{t("approvals.diffType")}</caption>
        <thead>
          <tr className="border-b border-border bg-surface-subtle text-xs font-medium text-surface-fg/70">
            <th scope="col" className="w-10 px-3 py-2 text-center">
              <span className="sr-only">{t("approvals.diffType")}</span>
            </th>
            <th scope="col" className="px-3 py-2">
              {t("approvals.field")}
            </th>
            <th scope="col" className="px-3 py-2">
              {t("approvals.before")}
            </th>
            <th scope="col" className="px-3 py-2">
              {t("approvals.after")}
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
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
              <tr
                key={`${field.path}-${idx}`}
                title={diffLabel}
                className={rowBg}
              >
                <td className="w-10 px-3 py-2 text-center font-mono font-bold select-none text-surface-fg">
                  <span aria-hidden="true">{marker}</span>
                  <span className="sr-only">{diffLabel}</span>
                </td>
                <td className="px-3 py-2 font-mono text-xs font-medium text-surface-fg align-top">
                  {field.path}
                </td>
                <td className="px-3 py-2 align-top">
                  <div className="max-w-xs overflow-x-auto md:max-w-sm lg:max-w-md">
                    {diffType === "added" ? (
                      <span className="text-surface-fg/40">—</span>
                    ) : (
                      <DiffValue value={field.from} />
                    )}
                  </div>
                </td>
                <td className="px-3 py-2 align-top">
                  <div className="max-w-xs overflow-x-auto md:max-w-sm lg:max-w-md">
                    {diffType === "removed" ? (
                      <span className="text-surface-fg/40">—</span>
                    ) : (
                      <DiffValue value={field.to} />
                    )}
                  </div>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
