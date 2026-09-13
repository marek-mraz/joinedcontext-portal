import { cloneElement, useId } from "react";
import type { JSX, ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../../api/permissions";
import type { Verb } from "../../api/permissions";

/**
 * Disabled with a reason (T-0603, UI-44, PF-50, PF-51): a control whose verb the caller's
 * effective permissions deny stays where it is, disabled, and says which verb on which kind
 * the role lacks, by pointer (the native tooltip) and by keyboard (a focusable wrapper the
 * reason describes). The guard reflects `permissions/me` and decides nothing: the same request
 * sent directly is the API's 403. While the document has not arrived, and for a bootstrap
 * administrator, the control renders as it is.
 */
export function PermissionGuard({
  project,
  kind,
  verb,
  children,
}: {
  project: string;
  kind: string;
  verb: Verb;
  /** One button or link; it receives `disabled` and `aria-disabled` when denied. */
  children: ReactElement<{ disabled?: boolean; "aria-disabled"?: boolean | "true"; "aria-describedby"?: string }>;
}): JSX.Element {
  const { t } = useTranslation();
  const { can } = usePermissions(project);
  const id = useId();
  if (can(kind, verb)) {
    return children;
  }
  const reason = t("permissions.denied", { verb, kind });
  return (
    <span tabIndex={0} title={reason} aria-describedby={id} className="inline-flex cursor-not-allowed">
      {cloneElement(children, { disabled: true, "aria-disabled": "true", "aria-describedby": id })}
      <span id={id} role="tooltip" className="sr-only">
        {reason}
      </span>
    </span>
  );
}
