import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { LifecycleBadge } from "./status/LifecycleBadge";
import type { Change } from "../api/manifest";
import { Alert, buttonClass } from "./ui";

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
    <Alert
      role="status"
      tone="info"
      actions={
        <Link
          to="/projects/$project/approvals/$id"
          params={{ project, id: change.metadata.name }}
          className={buttonClass("secondary", "sm")}
        >
          {t("changes.review")}
        </Link>
      }
    >
      <span className="flex flex-wrap items-center gap-2">
        <span>{t("changes.accepted")}</span>
        <span className="font-mono font-medium">{change.metadata.name}</span>
        <LifecycleBadge kind="lane" value={change.status.lane} />
        <LifecycleBadge kind="phase" value={change.status.phase} />
      </span>
    </Alert>
  );
}
