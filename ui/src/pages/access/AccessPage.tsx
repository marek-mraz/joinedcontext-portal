import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ServiceAccounts } from "./ServiceAccounts";
import { EffectivePermissions } from "./EffectivePermissions";

/** Project → Access: who is not a person here, and what any of us may actually read and write. */
export function AccessPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-8">
      <h1 className="text-xl font-bold">{t("access.title")}</h1>
      <ServiceAccounts project={project} />
      <EffectivePermissions project={project} />
    </div>
  );
}
