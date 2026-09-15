import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "@tanstack/react-router";
import { readCsrfToken } from "../../api/client";
import { handPrefill, takePrefill } from "../../assistant/state";
import { ChangeNotice } from "../../components/ChangeNotice";
import type { Change } from "../../api/manifest";
import type { Verdict } from "../../api/drafts";
import { parse as parseYaml } from "yaml";
import { Alert, Badge, Button, Card, Field, Icon, Input, PageHeader } from "../../components/ui";
import type { IconName } from "../../components/ui";

interface CompletedDraft {
  kind: string;
  name: string;
  inferred: boolean;
  manifest: Record<string, unknown>;
  verdict?: Verdict | null;
}

interface CompleteResult {
  space: string;
  found: string[];
  drafts: CompletedDraft[];
  proposeReady: boolean;
  lane: string;
  change?: Change | null;
}

const KIND_ICON: Record<string, IconName> = {
  DataModel: "models",
  ContextSpace: "spaces",
  DataSource: "datasources",
  Endpoint: "endpoints",
  Pipeline: "pipelines",
};

type Translate = (key: string, values?: Record<string, unknown>) => string;

/** What one draft is, in one line a person reads without knowing the kind: its type, feed, schedule or audience. */
function summaryOf(draft: CompletedDraft, t: Translate): string | null {
  const spec = (draft.manifest.spec ?? {}) as Record<string, unknown>;
  const text = (value: unknown): string | undefined => (typeof value === "string" && value !== "" ? value : undefined);
  switch (draft.kind) {
    case "DataModel": {
      const type = Array.isArray(spec.classes) ? text(spec.classes[0]) : undefined;
      if (type === undefined) return null;
      let attributes = 0;
      try {
        const source = parseYaml(text(spec.source) ?? "") as { classes?: Record<string, { attributes?: Record<string, unknown> }> } | null;
        attributes = Object.keys(source?.classes?.[type]?.attributes ?? {}).length;
      } catch {
        attributes = 0;
      }
      return t("spaces.complete.summary.DataModel", { type, count: attributes });
    }
    case "ContextSpace": {
      const model = text((spec.dataModelRef as { name?: unknown } | undefined)?.name);
      return model === undefined ? null : t("spaces.complete.summary.ContextSpace", { model });
    }
    case "DataSource": {
      const url = text((spec.http as { url?: unknown } | undefined)?.url);
      if (url === undefined) return null;
      try {
        return t("spaces.complete.summary.DataSource", { host: new URL(url).host });
      } catch {
        return null;
      }
    }
    case "Pipeline": {
      const type = text((spec.output as { type?: unknown } | undefined)?.type);
      const period = text(spec.period);
      return type === undefined || period === undefined ? null : t("spaces.complete.summary.Pipeline", { type, period });
    }
    case "Endpoint": {
      const audience = text(spec.audience);
      return audience === undefined ? null : t(`spaces.complete.summary.audience.${audience}`);
    }
    default:
      return null;
  }
}

/** The drafts the assistant's `space_complete` handed over with its navigation, when it did. */
function handedOver(prefill: Record<string, unknown> | null): { result: CompleteResult | null; url: string } {
  const result = prefill?.result as CompleteResult | undefined;
  return {
    result: result && Array.isArray(result.drafts) && typeof result.space === "string" ? result : null,
    url: typeof prefill?.url === "string" ? prefill.url : "",
  };
}

export function SpaceComplete({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // The agent already completed the space (AG-73): its drafts open here ready to propose.
  const [handed] = useState(() =>
    handedOver(typeof window === "undefined" ? null : takePrefill(window.location.pathname)),
  );

  const [spaceName, setSpaceName] = useState(() => {
    if (typeof window === "undefined") return "";
    return new URLSearchParams(window.location.search).get("space") ?? "";
  });
  const [url, setUrl] = useState(handed.url);
  const [files, setFiles] = useState<{ name: string; content: string }[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<CompleteResult | null>(handed.result);

  const handleFilesChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const selected = e.target.files;
    if (!selected) return;
    const loaded: { name: string; content: string }[] = [];
    for (let i = 0; i < selected.length; i++) {
      const f = selected[i];
      const text = await f.text();
      loaded.push({ name: f.name, content: text });
    }
    setFiles(loaded);
  };

  const executeComplete = async (propose: boolean) => {
    setLoading(true);
    setError(null);
    try {
      const headers: Record<string, string> = {
        "content-type": "application/json",
      };
      const csrf = readCsrfToken();
      if (csrf) {
        headers["x-csrf-token"] = csrf;
      }

      const payload: Record<string, unknown> = {
        propose,
      };
      if (spaceName.trim()) {
        payload.space = spaceName.trim();
      }
      if (url.trim()) {
        payload.url = url.trim();
      } else if (files.length > 0) {
        payload.files = files;
      }

      const res = await fetch(`/api/v1/projects/${encodeURIComponent(project)}/ops/jc_space_complete`, {
        method: "POST",
        credentials: "same-origin",
        headers,
        body: JSON.stringify(payload),
      });

      if (!res.ok) {
        const problem = (await res.json().catch(() => null)) as { detail?: string; error?: string } | null;
        setError(problem?.detail || problem?.error || `HTTP ${res.status}`);
        return;
      }

      const data = (await res.json()) as CompleteResult;
      setResult(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  // Each draft opens on its kind's page the way the assistant hands one over (AG-73).
  const openDraft = (d: CompletedDraft) => {
    const base = `/projects/${project}`;
    const draft = `?draft=${encodeURIComponent(d.name)}`;
    if (d.kind === "DataSource") {
      void navigate({ href: `${base}/datasources${draft}` });
    } else if (d.kind === "Pipeline") {
      void navigate({ href: `${base}/pipelines${draft}` });
    } else if (d.kind === "Endpoint") {
      void navigate({ href: `${base}/endpoints${draft}` });
    } else if (d.kind === "DataModel") {
      const source = (d.manifest.spec as { source?: unknown } | undefined)?.source;
      if (typeof source === "string") {
        handPrefill(`${base}/models`, { source });
      }
      void navigate({ href: `${base}/models` });
    } else {
      void navigate({ href: `${base}/spaces` });
    }
  };


  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("spaces.complete.title")}
        description={t("spaces.complete.lead")}
      />

      <div className="flex flex-col gap-4 rounded-md border border-border bg-surface p-4">
        <Field id="complete-space" label={t("spaces.complete.space")}>
          <Input
            id="complete-space"
            value={spaceName}
            onChange={(e) => setSpaceName(e.target.value)}
            placeholder={t("spaces.complete.spacePlaceholder")}
          />
        </Field>

        <Field id="complete-url" label={t("spaces.complete.url")}>
          <Input
            id="complete-url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder={t("spaces.complete.urlPlaceholder")}
          />
        </Field>

        <Field id="complete-files" label={t("spaces.complete.files")}>
          <input
            type="file"
            multiple
            id="complete-files"
            onChange={handleFilesChange}
            className="text-sm"
          />
          {files.length > 0 ? (
            <p className="mt-1 text-caption text-fg-muted">{files.map((f) => f.name).join(", ")}</p>
          ) : null}
        </Field>

        <div className="flex flex-wrap items-center gap-3">
          <Button
            id="complete-btn"
            variant={result?.proposeReady ? "secondary" : "primary"}
            disabled={loading || (!url.trim() && files.length === 0)}
            onClick={() => void executeComplete(false)}
          >
            {loading ? t("app.loading") : t("spaces.complete.action")}
          </Button>

          {result?.proposeReady ? (
            <Button
              id="complete-propose"
              variant="primary"
              disabled={loading}
              onClick={() => void executeComplete(true)}
            >
              {t("spaces.complete.proposeAll")}
            </Button>
          ) : null}
        </div>
      </div>

      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}

      {result?.change ? (
        <ChangeNotice change={result.change} project={project} />
      ) : null}

      {result?.drafts && result.drafts.length > 0 ? (
        <div className="flex flex-col gap-3">
          {result.drafts.map((d) => {
            const summary = summaryOf(d, t);
            return (
              <Card key={`${d.kind}-${d.name}`} data-testid={`complete-draft-${d.kind}`}>
                <div className="flex flex-wrap items-center justify-between gap-2 p-3">
                  <div className="flex min-w-0 flex-col gap-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <Icon name={KIND_ICON[d.kind] ?? "spaces"} className="size-4 text-fg-muted" />
                      <span className="font-semibold text-fg">
                        {KIND_ICON[d.kind] ? t(`spaces.complete.kind.${d.kind}`) : d.kind}
                      </span>
                      <span className="font-mono text-caption text-fg-muted">{d.name}</span>
                      <Badge tone={d.inferred ? "info" : "neutral"}>
                        {d.inferred ? t("spaces.complete.drafted") : t("spaces.complete.alreadyThere")}
                      </Badge>
                      {d.verdict ? (
                        <Badge tone={d.verdict.ok ? "success" : "danger"}>
                          {d.verdict.ok ? t("spaces.complete.checkPassed") : t("spaces.complete.checkFailed")}
                        </Badge>
                      ) : null}
                    </div>
                    {summary !== null ? <p className="text-caption text-fg-muted">{summary}</p> : null}
                  </div>
                  <Button size="sm" variant="ghost" onClick={() => openDraft(d)}>
                    {t("spaces.complete.open")}
                  </Button>
                </div>
                {d.verdict && !d.verdict.ok && d.verdict.findings.length > 0 ? (
                  <ul className="border-t border-border px-3 py-2 text-caption text-danger">
                    {d.verdict.findings.map((f, i) => (
                      <li key={i}>
                        {f.path ? `${f.path}: ` : ""}
                        {f.message}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </Card>
            );
          })}
          {result.proposeReady ? (
            <p className="text-caption text-fg-muted">{t("spaces.complete.onApproval")}</p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
