import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ServiceAccounts } from "./ServiceAccounts";
import { EffectivePermissions } from "./EffectivePermissions";
import { RoleBindings } from "./RoleBindings";
import { Roles } from "./Roles";
import { Groups } from "./Groups";
import { PageHeader } from "../../components/ui/PageHeader";

/** Project → Access: who holds which role here, who is not a person, and what any of us may actually read and write. */
export function AccessPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-8">
      <PageHeader title={t("access.title")} />
      <RoleBindings project={project} />
      <Roles project={project} />
      <Groups project={project} />
      <ServiceAccounts project={project} />
      <EffectivePermissions project={project} />
    </div>
  );
}
