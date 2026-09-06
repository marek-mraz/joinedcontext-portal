import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import * as Dialog from "@radix-ui/react-dialog";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import type { components } from "../../api/schema";

type KeyInfo = components["schemas"]["KeyInfo"];
type MintedKey = components["schemas"]["MintedKey"];

interface Credential {
  kind?: string;
  name?: string;
  expiresAt?: string;
}

interface ServiceAccountSpec {
  owner?: { user?: string };
  purpose?: string;
  roles?: { role?: string; scope?: Record<string, string>; types?: string[] }[];
  credentials?: Credential[];
}

/** `api-key` credentials only: an `oauth-client` lives in Keycloak and has no key here. */
function apiKeyCredentials(spec: ServiceAccountSpec): Credential[] {
  return (spec.credentials ?? []).filter((credential) => credential.kind === "api-key");
}

function formatDate(value: string | null | undefined, locale: string): string {
  if (!value) {
    return "";
  }
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleDateString(locale);
}

/**
 * The raw token, once. Everything about this dialog is built so that a person who closes it
 * without copying has lost the secret: there is no second read, and the Portal never had it
 * after the answer (PF-36).
 */
function TokenDialog({
  minted,
  onClose,
}: {
  minted: MintedKey | null;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  return (
    <Dialog.Root
      open={minted !== null}
      onOpenChange={(open) => {
        if (!open) {
          setCopied(false);
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 w-[min(40rem,92vw)] -translate-x-1/2 -translate-y-1/2 rounded border border-border bg-surface p-6 text-surface-fg shadow-lg">
          <Dialog.Title className="text-lg font-bold">{t("access.keys.newTitle")}</Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-surface-fg/70">
            {t("access.keys.newHint")}
          </Dialog.Description>

          <p
            role="alert"
            className="mt-4 rounded border border-danger bg-danger/10 p-3 text-sm text-danger"
          >
            {t("access.keys.onceWarning")}
          </p>

          <label className="mt-4 block text-sm font-medium" htmlFor="minted-token">
            {t("access.keys.token")}
          </label>
          <div className="mt-1 flex flex-wrap items-center gap-2">
            <input
              id="minted-token"
              readOnly
              value={minted?.token ?? ""}
              onFocus={(event) => event.currentTarget.select()}
              className="w-full flex-1 rounded border border-border bg-surface-subtle px-3 py-2 font-mono text-xs"
            />
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard
                  ?.writeText(minted?.token ?? "")
                  .then(() => setCopied(true))
                  .catch(() => setCopied(false));
              }}
              className="rounded border border-border px-3 py-1.5 text-sm font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {copied ? t("endpoints.copied") : t("access.keys.copy")}
            </button>
          </div>

          <Dialog.Close asChild>
            <button
              type="button"
              className="mt-6 rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
            >
              {t("access.keys.done")}
            </button>
          </Dialog.Close>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** The keys of one account: what exists, when it stops working, and the three actions on it. */
function KeyTable({
  project,
  account,
  credentials,
  onMinted,
  onError,
}: {
  project: string;
  account: string;
  credentials: Credential[];
  onMinted: (minted: MintedKey) => void;
  onError: (message: string | null) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState<string | null>(null);

  const keysKey = [...queryKeys.resource(project, "serviceaccounts", account), "keys"];
  const keys = useQuery({
    queryKey: keysKey,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/serviceaccounts/{name}/keys", {
          params: { path: { project, name: account } },
        }),
      ),
  });

  const failed = (err: unknown) => {
    onError(
      err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"),
    );
  };
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: keysKey });
  };

  const create = useMutation({
    mutationFn: async (credential: string) => {
      onError(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/serviceaccounts/{name}/keys", {
          params: { path: { project, name: account } },
          body: { credential },
        }),
      );
    },
    onSuccess: (minted) => {
      onMinted(minted);
      refresh();
    },
    onError: failed,
  });

  const rotate = useMutation({
    mutationFn: async (keyId: string) => {
      onError(null);
      return unwrap(
        await api.POST(
          "/api/v1/projects/{project}/serviceaccounts/{name}/keys/{keyId}/rotate",
          { params: { path: { project, name: account, keyId } }, body: {} },
        ),
      );
    },
    onSuccess: (minted) => {
      onMinted(minted);
      refresh();
    },
    onError: failed,
  });

  const revoke = useMutation({
    mutationFn: async (keyId: string) => {
      onError(null);
      const result = await api.DELETE(
        "/api/v1/projects/{project}/serviceaccounts/{name}/keys/{keyId}",
        { params: { path: { project, name: account, keyId } } },
      );
      if (result.error) {
        await unwrap(result as { error?: unknown; response: Response });
      }
      return keyId;
    },
    onSuccess: () => {
      setConfirming(null);
      refresh();
    },
    onError: failed,
  });

  const items: KeyInfo[] = keys.data?.items ?? [];
  const busy = create.isPending || rotate.isPending || revoke.isPending;

  return (
    <div className="mt-3 space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        {credentials.length === 0 ? (
          <p className="text-sm text-surface-fg/70">{t("access.keys.noCredential")}</p>
        ) : (
          credentials.map((credential) => (
            <button
              key={credential.name}
              type="button"
              disabled={busy}
              onClick={() => create.mutate(credential.name ?? "")}
              className="rounded bg-primary px-3 py-1.5 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2 disabled:opacity-50"
            >
              {t("access.keys.create", { credential: credential.name })}
            </button>
          ))
        )}
      </div>

      {keys.isError ? (
        <p role="status" className="text-sm text-surface-fg/70">
          {keys.error instanceof ApiError && keys.error.status === 503
            ? t("access.keys.noStore")
            : t("app.error.generic")}
        </p>
      ) : items.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("access.keys.empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left text-sm">
            <caption className="sr-only">
              {t("access.keys.tableCaption", { account })}
            </caption>
            <thead>
              <tr className="border-b border-border bg-surface-subtle">
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("access.keys.field.keyId")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("access.keys.field.credential")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("access.keys.field.expires")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium">
                  {t("access.keys.field.lastUsed")}
                </th>
                <th scope="col" className="px-4 py-2 font-medium text-right">
                  {t("approvals.actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {items.map((key) => (
                <tr key={key.keyId} className="hover:bg-surface-subtle/50">
                  <td className="px-4 py-3 font-mono text-xs">{key.keyId}</td>
                  <td className="px-4 py-3">{key.credential}</td>
                  <td className="px-4 py-3">
                    {key.revokedAt
                      ? t("access.keys.revoked", { date: formatDate(key.revokedAt, locale) })
                      : (formatDate(key.expiresAt, locale) || t("access.keys.never"))}
                  </td>
                  <td className="px-4 py-3">
                    {formatDate(key.lastUsedAt, locale) || t("access.keys.neverUsed")}
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex flex-wrap items-center justify-end gap-2">
                      {key.revokedAt ? null : (
                        <>
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => rotate.mutate(key.keyId)}
                            title={t("access.keys.rotateHint")}
                            className="rounded border border-border px-2.5 py-1 text-xs font-medium hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50"
                          >
                            {t("access.keys.rotate")}
                          </button>
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => setConfirming(key.keyId)}
                            className="rounded border border-danger px-2.5 py-1 text-xs font-medium text-danger hover:bg-danger/10 focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50"
                          >
                            {t("access.keys.revoke")}
                          </button>
                        </>
                      )}
                    </div>
                    {confirming === key.keyId ? (
                      <div
                        role="alertdialog"
                        aria-label={t("access.keys.revokeConfirm", { keyId: key.keyId })}
                        className="mt-2 flex flex-wrap items-center justify-end gap-2 rounded border border-danger bg-danger/10 p-2 text-xs"
                      >
                        <span>{t("access.keys.revokeConfirm", { keyId: key.keyId })}</span>
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => revoke.mutate(key.keyId)}
                          className="rounded bg-danger px-2.5 py-1 font-medium text-danger-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50"
                        >
                          {t("access.keys.revokeNow")}
                        </button>
                        <button
                          type="button"
                          onClick={() => setConfirming(null)}
                          className="rounded border border-border px-2.5 py-1 font-medium hover:bg-surface focus:outline-none focus:ring-2 focus:ring-border-focus"
                        >
                          {t("form.cancel")}
                        </button>
                      </div>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** Every non-human caller of one project: who owns it, what it may do, and its credentials. */
export function ServiceAccounts({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [minted, setMinted] = useState<MintedKey | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "serviceaccounts"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "serviceaccounts" } },
        }),
      ),
  });

  if (list.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (list.isError) {
    return (
      <div role="alert">
        <p className="text-danger">
          {list.error instanceof ApiError
            ? (list.error.problem?.detail ?? list.error.message)
            : t("app.error.generic")}
        </p>
        <button
          type="button"
          onClick={() => {
            void list.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  const accounts: Manifest[] = asManifests(list.data.items ?? []);

  return (
    <section className="space-y-4" aria-labelledby="service-accounts-heading">
      <h2 id="service-accounts-heading" className="text-lg font-bold">
        {t("access.accounts.title")}
      </h2>

      {error ? (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : null}

      {accounts.length === 0 ? (
        <p className="text-sm text-surface-fg/70">{t("access.accounts.empty")}</p>
      ) : (
        <ul className="space-y-4">
          {accounts.map((account) => {
            const spec = account.spec as ServiceAccountSpec;
            return (
              <li
                key={account.metadata.name}
                className="rounded border border-border p-4"
              >
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <h3 className="font-medium">
                    {localized(account.metadata.title, locale, account.metadata.name)}
                  </h3>
                  {account.metadata.title ? (
                    <span className="font-mono text-xs text-surface-fg/60">
                      {account.metadata.name}
                    </span>
                  ) : null}
                </div>
                <dl className="mt-2 grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
                  <div className="flex gap-2">
                    <dt className="text-surface-fg/60">{t("access.accounts.owner")}</dt>
                    <dd>{spec.owner?.user ?? ""}</dd>
                  </div>
                  <div className="flex gap-2">
                    <dt className="text-surface-fg/60">{t("access.accounts.roles")}</dt>
                    <dd>
                      {(spec.roles ?? [])
                        .map((role) => role.role)
                        .filter(Boolean)
                        .join(", ")}
                    </dd>
                  </div>
                  {spec.purpose ? (
                    <div className="flex gap-2 sm:col-span-2">
                      <dt className="text-surface-fg/60">{t("access.accounts.purpose")}</dt>
                      <dd>{spec.purpose}</dd>
                    </div>
                  ) : null}
                </dl>

                <KeyTable
                  project={project}
                  account={account.metadata.name}
                  credentials={apiKeyCredentials(spec)}
                  onMinted={setMinted}
                  onError={setError}
                />
              </li>
            );
          })}
        </ul>
      )}

      <TokenDialog minted={minted} onClose={() => setMinted(null)} />
    </section>
  );
}
