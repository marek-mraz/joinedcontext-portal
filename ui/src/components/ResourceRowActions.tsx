import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../api/permissions";
import { DeleteResourceAction } from "./DeleteResourceDialog";
import type { ResourceTarget } from "./DeleteResourceDialog";
import { EditResourceAction } from "./EditResourceDialog";
import type { EditableForm } from "./EditResourceDialog";
import { SaveAsResourceAction } from "./SaveAsDialog";
import { WorkOnCopyAction } from "./WorkOnCopyDialog";
import type { WorkOnCopyScope } from "./WorkOnCopyDialog";
import { RowActions } from "./ui/RowActions";
import type { RowAction } from "./ui/RowActions";

/**
 * The four actions every manifest row has, behind one menu (T-2287, T-2279, UI-26, UI-44).
 *
 * Every kind's table used to paint Edit, Save as, Work on a copy and Delete side by side, so four
 * controls competed with the data in every row of every list. This is that cell, once: the page's own
 * obvious action stays in the open as `primary`, the four go in the menu, and an action the role cannot
 * take stays listed, disabled, with the sentence that names the verb and the kind.
 *
 * The dialogs are rendered beside the menu and opened by the row's own state, because Radix unmounts a
 * menu's content when it closes and would take an open dialog with it.
 */
export function ResourceRowActions({
  project,
  target,
  scope,
  form,
  onEdit,
  primary,
}: {
  project: string;
  target: ResourceTarget;
  /** What "Work on a copy" copies; a kind that is not a space copies itself. */
  scope?: WorkOnCopyScope;
  /** The kind's own form for editing, when its page has one; without it the manifest opens as text. */
  form?: EditableForm;
  /** A page with its own editor (a pipeline, a data source) handles Edit itself instead. */
  onEdit?: () => void;
  primary?: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(target.home ?? project);
  const [openAction, setOpenAction] = useState<"edit" | "saveAs" | "copy" | "delete" | null>(null);
  const opens = (action: "edit" | "saveAs" | "copy" | "delete") => (open: boolean) =>
    setOpenAction(open ? action : null);
  const denied = (verb: "propose" | "delete") =>
    permissions.can(target.kind, verb) ? undefined : t("permissions.denied", { verb, kind: target.kind });

  const actions: RowAction[] = [
    {
      key: "edit",
      label: t("resourceEdit.button"),
      onSelect: onEdit ?? (() => setOpenAction("edit")),
      disabledReason: denied("propose"),
    },
    {
      key: "saveAs",
      label: t("saveAs.button"),
      onSelect: () => setOpenAction("saveAs"),
      disabledReason: denied("propose"),
    },
    {
      key: "copy",
      label: t("workspaces.open.action"),
      onSelect: () => setOpenAction("copy"),
    },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setOpenAction("delete"),
      disabledReason: denied("delete"),
    },
  ];

  return (
    <>
      <RowActions label={target.label ?? target.name} actions={actions} primary={primary} />
      {onEdit ? null : (
        <EditResourceAction
          target={target}
          form={form}
          trigger={false}
          open={openAction === "edit"}
          onOpenChange={opens("edit")}
        />
      )}
      <SaveAsResourceAction
        target={target}
        trigger={false}
        open={openAction === "saveAs"}
        onOpenChange={opens("saveAs")}
      />
      <WorkOnCopyAction
        project={project}
        scope={scope ?? { kind: "resources", items: [{ kind: target.kind, name: target.name }] }}
        trigger={false}
        open={openAction === "copy"}
        onOpenChange={opens("copy")}
      />
      <DeleteResourceAction
        target={target}
        trigger={false}
        open={openAction === "delete"}
        onOpenChange={opens("delete")}
      />
    </>
  );
}
