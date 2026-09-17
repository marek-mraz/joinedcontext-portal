import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import type { Change } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { ChangeNotice } from "../ChangeNotice";
import { Alert, Button, Dialog, Field, Icon, Input } from "../ui";

/** A DNS-1123 label, which is what a project slug is (PF-67). */
const LABEL = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** The organization's own namespace, which is never a project (PF-67). */
const ORG = "org";

/** What is wrong with the name the person is typing, live; `null` while it is fine. */
export function nameProblem(name: string): "empty" | "label" | "reserved" | null {
  if (name.trim() === "") {
    return "empty";
  }
  if (name === ORG) {
    return "reserved";
  }
  return LABEL.test(name) && name.length <= 63 ? null : "label";
}

/**
 * Opening a project from the sidebar (PF-65, PF-66, UI-44): the API says whether this caller may,
 * and the control is disabled with that reason rather than hidden. The result is the change a
 * person approves, or the project itself when the organization lets anyone open one.
 */
export function NewProjectDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [description, setDescription] = useState("");
  const [change, setChange] = useState<Change | null>(null);

  const problem = nameProblem(name);

  const openProject = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects", {
          body: {
            name,
            displayName: displayName.trim() === "" ? undefined : displayName,
            description: description.trim() === "" ? undefined : description,
          },
        }),
      ),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects() });
      // Nobody is waiting: the project is open, so the sidebar moves to it (PF-66).
      if (result.status.phase === "Merged") {
        close(false);
        void navigate({
          to: "/projects/$project/$plural",
          params: { project: name, plural: "spaces" },
        });
        return;
      }
      setChange(result);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setName("");
      setDisplayName("");
      setDescription("");
      setChange(null);
      openProject.reset();
    }
    onOpenChange(next);
  };

  const failure =
    openProject.error instanceof ApiError
      ? (openProject.error.problem?.detail ?? openProject.error.message)
      : openProject.error
        ? t("app.error.generic")
        : null;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("projects.newTitle")}
      description={t("projects.newLead")}
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
              variant="primary"
              disabled={problem !== null || openProject.isPending}
              onClick={() => openProject.mutate()}
            >
              {t("projects.open")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={name} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <Field
            id={`${ids}-name`}
            label={t("projects.nameLabel")}
            help={t("projects.nameHint")}
            errors={
              problem === "label"
                ? [t("projects.nameInvalid")]
                : problem === "reserved"
                  ? [t("projects.nameReserved")]
                  : undefined
            }
            required
          >
            <Input
              id={`${ids}-name`}
              value={name}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field id={`${ids}-display`} label={t("projects.displayNameLabel")}>
            <Input
              id={`${ids}-display`}
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </Field>
          <Field id={`${ids}-description`} label={t("projects.descriptionLabel")}>
            <Input
              id={`${ids}-description`}
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </Field>
        </div>
      )}
    </Dialog>
  );
}

/**
 * The sidebar's "New project" control. Who may open a project is the organization's own setting,
 * which `permissions/me` answers, and the refusal is shown on the control rather than by hiding
 * it (UI-44, PF-65). The API is what refuses; this only says so first.
 */
export function NewProjectButton({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  const [writing, setWriting] = useState(false);
  const creation = permissions.data?.projects?.creation;
  const allowed = creation?.allowed !== false;
  const reason = creation?.reason ?? t("projects.notAllowed");
  const button = (
    <Button
      variant="secondary"
      size="sm"
      className="w-full justify-center"
      disabled={!allowed}
      aria-disabled={allowed ? undefined : "true"}
      icon={<Icon name="plus" className="size-4" />}
      onClick={() => setWriting(true)}
    >
      {t("projects.new")}
    </Button>
  );
  return (
    <>
      {allowed ? (
        button
      ) : (
        <span tabIndex={0} title={reason} className="inline-flex w-full cursor-not-allowed">
          {button}
          <span role="tooltip" className="sr-only">
            {reason}
          </span>
        </span>
      )}
      <NewProjectDialog open={writing} onOpenChange={setWriting} />
    </>
  );
}
