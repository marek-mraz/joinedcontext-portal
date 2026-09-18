import { useId, useState } from "react";
import { PermissionGuard } from "./ui/PermissionGuard";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { isChange } from "../api/manifest";
import type { Change } from "../api/manifest";
import { usePermissions } from "../api/permissions";
import { ChangeNotice } from "./ChangeNotice";
import { Alert, Button, Dialog, Field, Input } from "./ui";

export interface ResourceTarget {
  project: string;
  /** The kind as the permissions name it, e.g. `Pipeline`. */
  kind: string;
  /** The plural of `/api/v1/projects/{project}/{plural}`. */
  plural: string;
  name: string;
  /** What the person calls it; the name when there is no title. */
  label?: string;
  /** Where its routes live when that is not the page's project: `org` for a Role or a RoleBinding. */
  home?: string;
}

/**
 * Removing a resource (AG-77, CC-19, CC-39): the name typed back, then one `DELETE` that opens
 * a Red change for an approver. Nothing is removed here; the answer is the change, or the
 * resources that still reference this one.
 */
export function DeleteResourceDialog({
  target,
  open,
  onOpenChange,
}: {
  target: ResourceTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const inputId = useId();
  const [typed, setTyped] = useState("");
  const [change, setChange] = useState<Change | null>(null);
  const { project, plural, name } = target;
  const home = target.home ?? project;

  const remove = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: home, plural, name } },
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  // Closing forgets what was typed and answered, so the next opening starts clean.
  const close = (next: boolean) => {
    if (!next) {
      setTyped("");
      setChange(null);
      remove.reset();
    }
    onOpenChange(next);
  };

  const failure =
    remove.error instanceof ApiError
      ? (remove.error.problem?.detail ?? remove.error.message)
      : remove.error
        ? t("app.error.generic")
        : null;
  const referenced = remove.error instanceof ApiError && remove.error.status === 409;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="sm"
      title={t("resourceDelete.title", { name: target.label ?? name })}
      description={t("resourceDelete.lead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button
              variant="danger"
              disabled={typed !== name || remove.isPending}
              onClick={() => remove.mutate()}
            >
              {t("resourceDelete.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          <Field id={inputId} label={t("resourceDelete.typeName", { name })}>
            <Input
              id={inputId}
              value={typed}
              // The one thing the dialog is for: typing the name back. Tabbing past the close
              // button and the heading to reach it is a keyboard tax nobody asked for (T-1054).
              autoFocus
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setTyped(event.target.value)}
            />
          </Field>
          {failure ? (
            <Alert tone="danger" role="alert" title={referenced ? t("resourceDelete.referenced") : undefined}>
              {failure}
            </Alert>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

/**
 * The Delete action of one row: shown only to a person whose role may delete the kind, the
 * dialog opened on click, or at once when the page was opened with `?delete=<name>`.
 */
export function DeleteResourceAction({ target }: { target: ResourceTarget }): JSX.Element {
  const { t } = useTranslation();
  const mayDelete = usePermissions(target.home ?? target.project).can(target.kind, "delete");
  const [open, setOpen] = useState(
    () =>
      typeof window !== "undefined" &&
      new URLSearchParams(window.location.search).get("delete") === target.name,
  );
  return (
    <>
      <PermissionGuard project={target.home ?? target.project} kind={target.kind} verb="delete">
        <Button
          size="sm"
          aria-label={t("resourceDelete.action", { name: target.label ?? target.name })}
          onClick={() => setOpen(true)}
        >
          {t("resourceDelete.button")}
        </Button>
      </PermissionGuard>
      {mayDelete ? <DeleteResourceDialog target={target} open={open} onOpenChange={setOpen} /> : null}
    </>
  );
}
