import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { components } from "../api/schema";
import { api, unwrap } from "../api/client";
import { Alert, Button, Dialog, Field, Input, Select } from "./ui";
import { useWorkspace } from "./layout/WorkspaceContext";

export interface WorkOnCopyScope {
  kind: "project" | "space" | "resources";
  name?: string;
  items?: { kind: string; name: string }[];
}

export interface WorkOnCopyDialogProps {
  project: string;
  scope: WorkOnCopyScope;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function defaultName(scope: WorkOnCopyScope): string {
  let raw: string;
  if (scope.kind === "space" && scope.name) {
    raw = scope.name;
  } else if (scope.kind === "resources" && scope.items?.length === 1) {
    raw = scope.items[0].name;
  } else {
    raw = "copy";
  }
  // Lowercase, replace non-alphanumeric/dash with dash, cut to 20.
  return raw
    .toLowerCase()
    .replace(/[^a-z0-9-]/g, "-")
    .slice(0, 20);
}

function isValidName(name: string): boolean {
  return /^[a-z0-9]([a-z0-9-]{0,18}[a-z0-9])?$/.test(name) && name.length <= 20;
}

export function WorkOnCopyDialog({
  project,
  scope,
  open,
  onOpenChange,
}: WorkOnCopyDialogProps): JSX.Element {
  const { t } = useTranslation();
  const { enter } = useWorkspace();
  const [name, setName] = useState(() => defaultName(scope));
  const [title, setTitle] = useState("");
  const [ttlDays, setTtlDays] = useState(7);
  const [error, setError] = useState<string | null>(null);

  const scopeText = useMemo(() => {
    if (scope.kind === "project") return t("workspaces.open.scopeProject");
    if (scope.kind === "space") return t("workspaces.open.scopeSpace", { name: scope.name });
    if (scope.kind === "resources" && scope.items) {
      return scope.items.map((i) => `${i.kind}/${i.name}`).join(", ");
    }
    return "";
  }, [scope, t]);

  const create = useMutation({
    mutationFn: async () => {
      const body: components["schemas"]["OpenRequest"] = {
        name,
        title: title.trim() || undefined,
        ttlDays,
        scope:
          scope.kind === "space"
            ? { kind: "space", name: scope.name ?? "" }
            : scope.kind === "resources"
              ? { kind: "resources", items: scope.items ?? [] }
              : { kind: "project" },
      };
      return unwrap(
        await api.POST("/api/v1/projects/{project}/workspaces", {
          params: { path: { project } },
          body,
        }),
      );
    },
    onSuccess: () => {
      enter(name);
      onOpenChange(false);
    },
    onError: (err: Error) => {
      setError(err.message);
    },
  });

  const canSubmit = isValidName(name) && !create.isPending;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setName(defaultName(scope));
          setTitle("");
          setTtlDays(7);
          setError(null);
        }
        onOpenChange(next);
      }}
      size="md"
      title={t("workspaces.open.title")}
      description={t("workspaces.open.description")}
      closeLabel={t("form.cancel")}
      footer={
        <>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t("form.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={!canSubmit}
            loading={create.isPending}
            onClick={() => create.mutate()}
          >
            {t("workspaces.open.submit")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {error ? (
          <Alert tone="danger" role="alert">
            {error}
          </Alert>
        ) : null}
        <Field id="ws-name" label={t("workspaces.open.name")} required>
          <Input
            id="ws-name"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setError(null);
            }}
            aria-describedby="ws-name-hint"
          />
          <p id="ws-name-hint" className="text-caption text-fg-muted">
            {t("workspaces.open.nameHint")}
          </p>
          {name && !isValidName(name) ? (
            <p role="alert" className="text-caption text-danger">
              {t("workspaces.open.nameInvalid")}
            </p>
          ) : null}
        </Field>
        <Field id="ws-title" label={t("workspaces.open.title")}>
          <Input
            id="ws-title"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
        </Field>
        <Field id="ws-scope" label={t("workspaces.open.scope")}>
          <p className="text-body text-fg-muted">{scopeText}</p>
        </Field>
        <Field id="ws-ttl" label={t("workspaces.open.ttl")}>
          <Select
            id="ws-ttl"
            value={ttlDays}
            onChange={(e) => setTtlDays(Number(e.target.value))}
          >
            {[1, 3, 7, 14].map((d) => (
              <option key={d} value={d}>
                {t("workspaces.open.ttlDays", { days: d })}
              </option>
            ))}
          </Select>
        </Field>
      </div>
    </Dialog>
  );
}

export function WorkOnCopyAction({
  project,
  scope,
  label,
  variant = "secondary",
}: {
  project: string;
  scope: WorkOnCopyScope;
  label?: string;
  variant?: "primary" | "secondary";
}): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant={variant} size="sm" onClick={() => setOpen(true)}>
        {label ?? t("workspaces.open.action")}
      </Button>
      {open ? (
        <WorkOnCopyDialog
          project={project}
          scope={scope}
          open={open}
          onOpenChange={setOpen}
        />
      ) : null}
    </>
  );
}
