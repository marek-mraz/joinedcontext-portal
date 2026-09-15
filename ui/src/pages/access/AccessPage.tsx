import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ServiceAccounts } from "./ServiceAccounts";
import { EffectivePermissions } from "./EffectivePermissions";
import { RoleBindings } from "./RoleBindings";

/** Project → Access: who holds which role here, who is not a person, and what any of us may actually read and write. */
export function AccessPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-8">
      <h1 className="text-xl font-bold">{t("access.title")}</h1>
      <RoleBindings project={project} />
      <ServiceAccounts project={project} />
      <EffectivePermissions project={project} />
    </div>
  );
}
