import { useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { readCsrfToken } from "../../api/client";
import type { ProblemDetails } from "../../api/client";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Button, Field, Input, PageHeader, Select } from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import type { components } from "../../api/schema";

type ImportReport = components["schemas"]["ImportReport"];

/** What the import does with a resource the project already has (MF-23). */
const POLICIES = ["fail", "skip", "replace", "rename"] as const;
type Policy = (typeof POLICIES)[number];

/**
 * The keys whose string value is a secret somebody pasted instead of referencing (MF-24).
 *
 * The same list the Portal refuses on, `SECRET_KEYS` in `src/api/mutate.rs`. It is here so the
 * person is told before the upload leaves the browser rather than after the whole bundle is
 * refused; the server's refusal is the one that counts and is not weakened by this.
 */
const SECRET_KEYS = [
  "password",
  "token",
  "secret",
  "clientSecret",
  "apiKey",
  "client_secret",
  "api_key",
  "apiToken",
  "api_token",
];

/**
 * A literal secret in the text of a bundle, as `key: value` on one line.
 *
 * Text rather than parsed YAML: an archive is bytes, a stream is many documents, and the check
 * is a courtesy before the upload, not the gate. `secretRef:` and any other key whose value is
 * an object are left alone — a reference is what the manifest is supposed to carry.
 */
export function pastedSecrets(text: string): string[] {
  const found: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const match = /^\s*(?:-\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(\S.*)$/.exec(line);
    if (!match) {
      continue;
    }
    const [, key, value] = match;
    const written = value.trim();
    // An object or an empty value is a reference or a nested block, not a pasted credential.
    if (!SECRET_KEYS.includes(key) || written === "" || written.startsWith("{")) {
      continue;
    }
    if (!found.includes(key)) {
      found.push(key);
    }
  }
  return found;
}

/** The body an import posts: the file and the options beside it (API/01 §4). */
function form(file: File, namespace: string, orgDomain: string, policy: Policy): FormData {
  const body = new FormData();
  body.append("file", file);
  if (namespace.trim()) {
    body.append("targetNamespace", namespace.trim());
  }
  if (orgDomain.trim()) {
    body.append("orgDomain", orgDomain.trim());
  }
  body.append("conflictPolicy", policy);
  return body;
}

/**
 * Import a bundle into a project (MF-20…MF-24, MF-33).
 *
 * Two steps, and the first is not optional in practice: the check posts the bundle with
 * `?dryRun=All` and the person reads what it would do — created, replaced, skipped, renamed, and
 * whether each file still equals the checksum the bundle carried (MF-42). Only then is there a
 * button that proposes, and what it proposes is one Change for the whole bundle (CC-32).
 *
 * There is no URL field. MF-20 allows one and the Portal refuses it with `501`, because fetching
 * a host a caller names is an egress decision with no policy behind it; a field whose only
 * outcome is that refusal would be a worse answer than saying so once.
 */
export function ImportPage({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const [file, setFile] = useState<File | null>(null);
  const [namespace, setNamespace] = useState(project);
  const [orgDomain, setOrgDomain] = useState("");
  const [policy, setPolicy] = useState<Policy>("fail");
  const [pasted, setPasted] = useState<string[]>([]);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const post = async (dryRun: boolean) => {
    if (!file) {
      throw new Error(t("import.noFile"));
    }
    const headers: Record<string, string> = {};
    const csrf = readCsrfToken();
    if (csrf) {
      headers["x-csrf-token"] = csrf;
    }
    const response = await fetch(
      `/api/v1/projects/${encodeURIComponent(project)}/import${dryRun ? "?dryRun=All" : ""}`,
      { method: "POST", credentials: "same-origin", headers, body: form(file, namespace, orgDomain, policy) },
    );
    const answered = (await response.json().catch(() => ({}))) as ProblemDetails & Record<string, unknown>;
    if (!response.ok) {
      // The refusal is the Portal's own sentence — which manifest and why (MF-24) — shown as it
      // came rather than replaced with a summary of it.
      throw new Error(answered.detail ?? answered.title ?? `HTTP ${response.status}`);
    }
    return answered;
  };

  const check = useMutation({
    mutationFn: () => post(true),
    onMutate: () => {
      setProblem(null);
      setChange(null);
    },
    onSuccess: (answered) => setReport(answered as unknown as ImportReport),
    onError: (error: Error) => {
      setReport(null);
      setProblem(error.message);
    },
  });

  const propose = useMutation({
    mutationFn: () => post(false),
    onMutate: () => setProblem(null),
    onSuccess: (answered) => setChange(answered as unknown as Change),
    onError: (error: Error) => setProblem(error.message),
  });

  const take = async (chosen: File | null) => {
    setFile(chosen);
    setReport(null);
    setChange(null);
    setProblem(null);
    setPasted([]);
    if (!chosen) {
      return;
    }
    // A zip is bytes; only a text bundle can be read for pasted credentials here, and the
    // server checks both.
    if (/\.(ya?ml|json)$/i.test(chosen.name)) {
      setPasted(pastedSecrets(await chosen.text()));
    }
  };

  const counted = (report: ImportReport) =>
    [
      t("import.report.created", { count: report.created.length }),
      t("import.report.replaced", { count: report.replaced.length }),
      t("import.report.skipped", { count: report.skipped.length }),
      t("import.report.renamed", { count: Object.keys(report.renamed ?? {}).length }),
      t("import.report.files", { count: report.nativeFiles }),
    ].join(" · ");

  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("import.title")} description={t("import.lead")} />

      <section aria-labelledby="import-bundle" className="flex flex-col gap-3 rounded border border-border p-4">
        <h2 id="import-bundle" className="text-base font-semibold">
          {t("import.bundle")}
        </h2>
        <Field id="import-file" label={t("import.file")} help={t("import.fileHelp")}>
          <input
            id="import-file"
            type="file"
            accept=".zip,.yaml,.yml,.json"
            onChange={(event) => void take(event.target.files?.[0] ?? null)}
          />
        </Field>
        <p className="text-caption text-fg-muted">{t("import.noUrl")}</p>

        {pasted.length > 0 ? (
          <Alert role="alert" tone="danger">
            {t("import.pastedSecret", { keys: pasted.join(", ") })}
          </Alert>
        ) : null}

        <div className="flex flex-wrap items-end gap-4">
          <Field id="import-namespace" label={t("import.namespace")} help={t("import.namespaceHelp")}>
            <Input
              id="import-namespace"
              value={namespace}
              onChange={(event) => setNamespace(event.target.value)}
            />
          </Field>
          <Field id="import-domain" label={t("import.orgDomain")} help={t("import.orgDomainHelp")}>
            <Input
              id="import-domain"
              value={orgDomain}
              placeholder={t("import.orgDomainOwn")}
              onChange={(event) => setOrgDomain(event.target.value)}
            />
          </Field>
          <Field id="import-policy" label={t("import.policy")} help={t(`import.policyHelp.${policy}`)}>
            <Select
              id="import-policy"
              value={policy}
              onChange={(event) => setPolicy(event.target.value as Policy)}
            >
              {POLICIES.map((name) => (
                <option key={name} value={name}>
                  {t(`import.policyOption.${name}`)}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Button
            onClick={() => check.mutate()}
            disabled={!file || pasted.length > 0}
            submitting={check.isPending}
          >
            {t("import.check")}
          </Button>
          <PermissionGuard project={project} kind="*" verb="propose">
            <Button
              variant="primary"
              onClick={() => propose.mutate()}
              disabled={!report || pasted.length > 0}
              submitting={propose.isPending}
            >
              {t("import.propose")}
            </Button>
          </PermissionGuard>
          {!report && file ? (
            <span role="status" className="text-caption text-fg-muted">
              {t("import.checkFirst")}
            </span>
          ) : null}
        </div>
      </section>

      {problem ? (
        <Alert role="alert" tone="danger">
          {problem}
        </Alert>
      ) : null}

      {report ? (
        <section aria-labelledby="import-report" className="flex flex-col gap-2 rounded border border-border p-4">
          <h2 id="import-report" className="text-base font-semibold">
            {t("import.report.title")}
          </h2>
          <p className="text-body">{counted(report)}</p>
          <p className="text-caption text-fg-muted">
            {t("import.report.lane", { lane: t(`lane.${report.lane}`) })}
          </p>
          {report.source ? (
            <p className="text-caption text-fg-muted">{t("import.report.source", { source: report.source })}</p>
          ) : null}
          {(["created", "replaced", "skipped"] as const).map((group) =>
            report[group].length > 0 ? (
              <details key={group}>
                <summary className="cursor-pointer text-caption text-fg-muted">
                  {t(`import.report.${group}`, { count: report[group].length })}
                </summary>
                <ul className="mt-1 list-disc pl-5 text-caption">
                  {report[group].map((name) => (
                    <li key={name} className="font-mono">
                      {name}
                    </li>
                  ))}
                </ul>
              </details>
            ) : null,
          )}
          {Object.keys(report.renamed ?? {}).length > 0 ? (
            <details>
              <summary className="cursor-pointer text-caption text-fg-muted">
                {t("import.report.renamed", { count: Object.keys(report.renamed).length })}
              </summary>
              <ul className="mt-1 list-disc pl-5 text-caption">
                {Object.entries(report.renamed).map(([from, to]) => (
                  <li key={from} className="font-mono">
                    {from} → {to}
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
          {/* MF-42: a transfer that carried no checksums says so, rather than reading as if
              every file were equal. */}
          {report.verified && report.verified.length > 0 ? (
            <p className="text-caption text-fg-muted">
              {t("import.report.verified", {
                equal: report.verified.filter((file) => file.equal).length,
                total: report.verified.length,
              })}
            </p>
          ) : (
            <p className="text-caption text-fg-muted">{t("import.report.unverifiable")}</p>
          )}
        </section>
      ) : null}

      {change ? <ChangeNotice change={change} project={project} /> : null}
    </div>
  );
}
