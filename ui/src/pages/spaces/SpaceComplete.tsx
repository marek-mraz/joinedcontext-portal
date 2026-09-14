import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "@tanstack/react-router";
import { readCsrfToken } from "../../api/client";
import { ChangeNotice } from "../../components/ChangeNotice";
import type { Change } from "../../api/manifest";
import type { Verdict } from "../../api/drafts";
import { Alert, Badge, Button, Card, Field, Input, PageHeader } from "../../components/ui";

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

export function SpaceComplete({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const [spaceName, setSpaceName] = useState(() => {
    if (typeof window === "undefined") return "";
    return new URLSearchParams(window.location.search).get("space") ?? "";
  });
  const [url, setUrl] = useState("");
  const [files, setFiles] = useState<{ name: string; content: string }[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<CompleteResult | null>(null);

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

  const openDraft = (d: CompletedDraft) => {
    if (d.kind === "DataSource") {
      void navigate({ to: `/projects/${project}/datasources?draft=${encodeURIComponent(d.name)}` });
    } else if (d.kind === "Pipeline") {
      void navigate({ to: `/projects/${project}/pipelines?draft=${encodeURIComponent(d.name)}` });
    } else if (d.kind === "DataModel") {
      const linkmlSource = (d.manifest.spec as { source?: string })?.source;
      if (linkmlSource) {
        sessionStorage.setItem(`jc_prefill:/projects/${project}/models`, JSON.stringify({ source: linkmlSource }));
      }
      void navigate({ to: `/projects/${project}/models` });
    } else {
      void navigate({ to: `/projects/${project}/spaces` });
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
            placeholder="e.g. bikes"
          />
        </Field>

        <Field id="complete-url" label={t("spaces.complete.url")}>
          <Input
            id="complete-url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://.../station_status.json"
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

        <div className="flex items-center gap-3">
          <Button
            id="complete-btn"
            variant="primary"
            disabled={loading || (!url.trim() && files.length === 0)}
            onClick={() => void executeComplete(false)}
          >
            {loading ? t("app.loading") : t("spaces.complete.action")}
          </Button>

          {result?.proposeReady ? (
            <Button
              id="complete-propose"
              variant="secondary"
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
          {result.drafts.map((d) => (
            <Card key={`${d.kind}-${d.name}`} data-testid={`complete-draft-${d.kind}`}>
              <div className="flex flex-wrap items-center justify-between gap-2 p-3">
                <div className="flex items-center gap-2">
                  <span className="font-semibold text-fg">{d.kind}</span>
                  <span className="font-mono text-caption text-fg-muted">{d.name}</span>
                  <Badge tone={d.inferred ? "warning" : "info"}>
                    {d.inferred ? t("spaces.complete.inferred") : t("spaces.complete.found")}
                  </Badge>
                  {d.verdict ? (
                    <Badge tone={d.verdict.ok ? "success" : "danger"}>
                      {d.verdict.ok ? "green" : "red"}
                    </Badge>
                  ) : null}
                </div>
                <Button size="sm" variant="ghost" onClick={() => openDraft(d)}>
                  {t("spaces.complete.open")}
                </Button>
              </div>
              {d.verdict && !d.verdict.ok && d.verdict.findings.length > 0 ? (
                <div className="border-t border-border px-3 py-2 text-caption text-danger">
                  {d.verdict.findings.map((f, i) => (
                    <div key={i}>{f.path ? `${f.path}: ` : ""}{f.message}</div>
                  ))}
                </div>
              ) : null}
            </Card>
          ))}
        </div>
      ) : null}
    </div>
  );
}
