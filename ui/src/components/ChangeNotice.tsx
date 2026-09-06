import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { LifecycleBadge } from "./status/LifecycleBadge";
import type { Change } from "../api/manifest";

/** What a write leaves behind: a merge request waiting for an approver, not a saved record. */
export function ChangeNotice({
  change,
  project,
}: {
  change: Change;
  project: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-2 rounded border border-border bg-surface-subtle p-3 text-sm"
    >
      <span>{t("changes.accepted")}</span>
      <span className="font-mono">{change.metadata.name}</span>
      <LifecycleBadge kind="lane" value={change.status.lane} />
      <LifecycleBadge kind="phase" value={change.status.phase} />
      <Link
        to="/projects/$project/approvals/$id"
        params={{ project, id: change.metadata.name }}
        className="text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
      >
        {t("changes.review")}
      </Link>
    </div>
  );
}
