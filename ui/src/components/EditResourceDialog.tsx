import { Suspense, lazy, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { isChange } from "../api/manifest";
import type { Change } from "../api/manifest";
import { usePermissions } from "../api/permissions";
import { takeEditRequest } from "../assistant/state";
import { ChangeNotice } from "./ChangeNotice";
import type { ResourceTarget } from "./DeleteResourceDialog";
import { Alert, Button, Dialog } from "./ui";

const MonacoSourceView = lazy(() => import("../pages/models/MonacoSourceView"));

/** The manifest as it is written: status is the Portal's to compute and never goes back (MF-04). */
function writable(manifest: Record<string, unknown>): string {
  const rest = { ...manifest };
  delete rest.status;
  return stringifyYaml(rest);
}

/**
 * Editing a resource that has no form of its own (AG-77, CC-19): its manifest as YAML, then one
 * `PUT` that opens a change for an approver. The name stays; a renamed manifest is a new resource.
 */
export function EditResourceDialog({
  target,
  open,
  onOpenChange,
  changed,
}: {
  target: ResourceTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The manifest with a change already made, e.g. by the assistant; shown in place of the stored one. */
  changed?: Record<string, unknown> | null;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { project, plural, name } = target;
  const [text, setText] = useState<string | null>(() => (changed ? writable(changed) : null));
  const [invalid, setInvalid] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const current = useQuery({
    queryKey: [...queryKeys.list(project, plural), name],
    enabled: open,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural, name } },
        }),
      ),
  });
  const source = text ?? (current.data ? writable(current.data as Record<string, unknown>) : "");

  const propose = useMutation({
    mutationFn: async (body: unknown) =>
      unwrap(
        await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural, name } },
          body: body as never,
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const submit = () => {
    let manifest: unknown;
    try {
      manifest = parseYaml(source);
    } catch (error) {
      setInvalid(error instanceof Error ? error.message : String(error));
      return;
    }
    const written = (manifest as { metadata?: { name?: unknown } } | null)?.metadata?.name;
    if (written !== name) {
      setInvalid(t("resourceEdit.renamed", { name }));
      return;
    }
    setInvalid(null);
    propose.mutate(manifest);
  };

  // Closing forgets the edit and the answer, so the next opening starts from the stored manifest.
  const close = (next: boolean) => {
    if (!next) {
      setText(null);
      setInvalid(null);
      setChange(null);
      propose.reset();
    }
    onOpenChange(next);
  };

  const failure =
    invalid ??
    (propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error || current.error
        ? t("app.error.generic")
        : null);

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="lg"
      title={t("resourceEdit.title", { name: target.label ?? name })}
      description={t("resourceEdit.lead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button variant="primary" disabled={!current.data || propose.isPending} onClick={submit}>
              {t("resourceEdit.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <div className="overflow-hidden rounded-md border border-border">
            {current.data ? (
              <Suspense fallback={<p className="p-3 text-body">{t("models.loadingEditor")}</p>}>
                <MonacoSourceView
                  value={source}
                  onChange={(next) => {
                    setText(next);
                    setInvalid(null);
                  }}
                  onMount={() => undefined}
                  height="24rem"
                />
              </Suspense>
            ) : (
              <p className="p-3 text-body">{t("app.loading")}</p>
            )}
          </div>
        </div>
      )}
    </Dialog>
  );
}

/**
 * The Edit action of one row: shown only to a person whose role may propose the kind, the dialog
 * opened on click, or at once when the page was opened with `?edit=<name>`, on the change the
 * assistant made when it made one.
 */
export function EditResourceAction({ target }: { target: ResourceTarget }): JSX.Element | null {
  const { t } = useTranslation();
  const mayPropose = usePermissions(target.project).can(target.kind, "propose");
  const [request] = useState(() => takeEditRequest(target.name));
  const [open, setOpen] = useState(request !== null);
  if (!mayPropose) {
    return null;
  }
  return (
    <>
      <Button
        size="sm"
        aria-label={t("resourceEdit.action", { name: target.label ?? target.name })}
        onClick={() => setOpen(true)}
      >
        {t("resourceEdit.button")}
      </Button>
      <EditResourceDialog target={target} open={open} onOpenChange={setOpen} changed={request?.manifest} />
    </>
  );
}
