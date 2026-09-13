import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { readCsrfToken } from "../../api/client";
import { Alert, Button } from "../../components/ui";
import type { PipelineForm } from "./PipelineEditor";

/**
 * A candidate pipeline tried on a sample before it is proposed (PL-43, PL-44, UI-32): a dropped
 * CSV or JSON drafts the type and the Bloblang mapping, one click runs the mapping where it
 * will run (the project's runner, through the Portal), and the trace comes back as stages:
 * the input, what the mapping made of it, whether that is an entity, and every error at its
 * line. The sample stays in memory and in this request; nothing is written anywhere.
 */

/** The largest sample the route takes (API/01 §7a). */
export const MAX_SAMPLE_BYTES = 5 * 1024 * 1024;

export type SampleFormat = "csv" | "json" | "text";

export interface Trace {
  input: { events: number; bytes: number; sample?: unknown };
  mapping: unknown[];
  validation: { index: number; ok: boolean; problems: string[] }[];
  errors: { stage: string; line?: number | null; message: string }[];
}

export interface Draft {
  format: SampleFormat;
  type: string;
  idColumn: string;
  columns: string[];
  bloblang: string;
}

/** The format a file name says, then what its first bytes say. */
export function formatOf(name: string, text: string): SampleFormat {
  const extension = name.toLowerCase().split(".").pop() ?? "";
  if (extension === "csv") {
    return "csv";
  }
  if (extension === "json") {
    return "json";
  }
  const head = text.trimStart();
  if (head.startsWith("{") || head.startsWith("[")) {
    return "json";
  }
  return text.split("\n", 1)[0]?.includes(",") ? "csv" : "text";
}

/** `air_quality-2026.csv` becomes `AirQuality2026`: an entity type short name (PF-42). */
export function typeOf(name: string): string {
  const stem = name.replace(/\.[^.]*$/, "");
  const words = stem.split(/[^A-Za-z0-9]+/).filter(Boolean);
  const type = words.map((w) => w[0].toUpperCase() + w.slice(1)).join("");
  return /^[A-Z]/.test(type) ? type : `Sample${type}`;
}

/** The column a local id comes from (PL-44): `id`, `identifier`, one ending in `_id`/`Id`, or the first. */
export function idColumnOf(columns: string[]): string {
  const lower = columns.map((c) => c.toLowerCase());
  const exact = lower.findIndex((c) => c === "id" || c === "identifier");
  if (exact >= 0) {
    return columns[exact];
  }
  const suffix = columns.findIndex((c) => /(_id|Id|ID)$/.test(c));
  return columns[suffix >= 0 ? suffix : 0] ?? "id";
}

function isNumeric(value: unknown): boolean {
  return typeof value === "number" || (typeof value === "string" && value.trim() !== "" && !Number.isNaN(Number(value)));
}

function splitCsvLine(line: string): string[] {
  const delimiter = line.includes(";") && !line.includes(",") ? ";" : ",";
  return line.split(delimiter).map((cell) => cell.trim().replace(/^"(.*)"$/, "$1"));
}

/** The first record of the sample: column names and one value each, to see which are numbers. */
export function firstRecordOf(text: string, format: SampleFormat): Record<string, unknown> | null {
  if (format === "csv") {
    const [header, first] = text.split(/\r?\n/).filter((line) => line.trim() !== "");
    if (!header) {
      return null;
    }
    const columns = splitCsvLine(header);
    const values = first ? splitCsvLine(first) : [];
    return Object.fromEntries(columns.map((column, i) => [column, values[i] ?? ""]));
  }
  if (format === "json") {
    try {
      const parsed: unknown = JSON.parse(text);
      const record = Array.isArray(parsed) ? parsed[0] : parsed;
      return typeof record === "object" && record !== null && !Array.isArray(record)
        ? (record as Record<string, unknown>)
        : null;
    } catch {
      return null;
    }
  }
  return null;
}

/**
 * The mapping a sample drafts (PL-44): the id minted from the id column under the
 * organization's domain and the target space, the type, and one Property per other column,
 * as a number where the sample holds one.
 */
export function draftFromSample(name: string, text: string, space: string): Draft | null {
  const format = formatOf(name, text);
  const record = firstRecordOf(text, format);
  if (!record) {
    return null;
  }
  const columns = Object.keys(record).filter((c) => c !== "");
  if (columns.length === 0) {
    return null;
  }
  const type = typeOf(name);
  const idColumn = idColumnOf(columns);
  const path = (column: string) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(column) ? `this.${column}` : `this.${JSON.stringify(column)}`);
  const lines = [
    `# drafted from ${name}: ${columns.length} columns, id from ${idColumn} (PL-44)`,
    `let domain = env("JC_ORG_DOMAIN")`,
    `root.id = "urn:ngsi-ld:%v:%v:%v:%v".format(${JSON.stringify(type)}, $domain, ${JSON.stringify(space)}, ${path(idColumn)}.string())`,
    `root.type = ${JSON.stringify(type)}`,
    ...columns
      .filter((column) => column !== idColumn)
      .map((column) => {
        const value = isNumeric(record[column]) ? `${path(column)}.number().catch(${path(column)})` : path(column);
        const property = column.replace(/[^A-Za-z0-9_]/g, "_");
        return `root.${property} = { "type": "Property", "value": ${value} }`;
      }),
    "",
  ];
  return { format, type, idColumn, columns, bloblang: lines.join("\n") };
}

/** The space of `urn:ngsi-ld:Endpoint:{org}:{space}:{name}`, or the fallback. */
export function spaceOfTarget(target: string | undefined, fallback: string): string {
  const parts = target?.split(":") ?? [];
  return parts.length === 6 && parts[2] === "Endpoint" && parts[4] ? parts[4] : fallback;
}

/** The lines of the mapping an error names, 1-based. */
export function errorLines(trace: Trace): Set<number> {
  return new Set(trace.errors.flatMap((e) => (typeof e.line === "number" && e.line > 0 ? [e.line] : [])));
}

function pretty(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsText(file);
  });
}

export interface PipelineTestProps {
  project: string;
  draft: PipelineForm | undefined;
  onChange: (form: PipelineForm) => void;
  /** The manifest the form is right now, as the dialog proposes it. */
  toManifest: (form: PipelineForm) => unknown;
  /** The `http` DataSource's own URL, offered as the sample the runner fetches (PL-48). */
  sampleUrl?: string;
  /** The test's answer for the mapping it ran (PL-49). */
  onVerdict?: (ok: boolean, bloblang: string) => void;
  /** Notifies whenever the test trace is produced or cleared. */
  onTrace?: (trace: Trace | null) => void;
}

/** A sample is a file held in memory, or a URL the runner fetches itself (PL-43, PL-48). */
type SampleSource = { name: string; format: SampleFormat } & ({ text: string; url?: undefined } | { url: string; text?: undefined });

export function PipelineTest({ project, draft, onChange, toManifest, sampleUrl, onVerdict, onTrace }: PipelineTestProps): JSX.Element {
  const { t } = useTranslation();
  const [sample, setSample] = useState<SampleSource | null>(null);
  const [trace, setTrace] = useState<Trace | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const bloblang = draft?.compute?.bloblang ?? "";

  async function takeFile(file: File | undefined) {
    if (!file) {
      return;
    }
    setTrace(null);
    onTrace?.(null);
    setError(null);
    if (file.size > MAX_SAMPLE_BYTES) {
      setSample(null);
      setError(t("pipelines.test.tooLarge"));
      return;
    }
    const text = await readText(file);
    const space = spaceOfTarget(draft?.targetEndpoint, project);
    const drafted = draftFromSample(file.name, text, space);
    setSample({ name: file.name, text, format: drafted?.format ?? formatOf(file.name, text) });
    if (drafted) {
      const form = draft ?? { class: "auto" };
      onChange({
        ...form,
        compute: { kind: "bloblang", bloblang: drafted.bloblang },
        output: { type: drafted.type, mode: form.output?.mode ?? "upsert" },
      });
    }
  }

  async function run() {
    if (!sample || !draft) {
      return;
    }
    setRunning(true);
    setError(null);
    try {
      const response = await fetch(`/api/v1/projects/${encodeURIComponent(project)}/pipelines/test`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json", "x-csrf-token": readCsrfToken() ?? "" },
        body: JSON.stringify({
          pipeline: toManifest(draft),
          sample:
            sample.url !== undefined
              ? { url: sample.url, format: sample.format }
              : { text: sample.text, format: sample.format },
        }),
      });
      if (!response.ok) {
        const problem = (await response.json().catch(() => null)) as { detail?: string } | null;
        setTrace(null);
        onTrace?.(null);
        setError(problem?.detail ?? t("pipelines.test.failed", { status: response.status }));
        return;
      }
      const answer = (await response.json()) as Trace;
      setTrace(answer);
      onTrace?.(answer);
      onVerdict?.(
        answer.errors.length === 0 && answer.validation.length > 0 && answer.validation.every((v) => v.ok),
        bloblang,
      );
    } catch {
      setTrace(null);
      onTrace?.(null);
      setError(t("pipelines.test.failed", { status: 0 }));
    } finally {
      setRunning(false);
    }
  }

  const lines = trace ? errorLines(trace) : new Set<number>();
  const ready = trace !== null && trace.errors.length === 0 && trace.validation.length > 0 && trace.validation.every((v) => v.ok);
  const panel = "flex min-w-0 flex-col gap-1 rounded-md border border-border bg-surface p-2";
  const pre = "max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono text-caption";

  return (
    <section
      className="flex flex-col gap-3 rounded-md border border-border bg-surface-subtle p-3"
      aria-labelledby="studio-test"
      data-testid="pipeline-test"
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => {
        event.preventDefault();
        void takeFile(event.dataTransfer.files[0]);
      }}
    >
      <h3 id="studio-test" className="text-body font-semibold text-fg">
        {t("pipelines.test.title")}
      </h3>
      <p className="text-caption text-fg-muted">{t("pipelines.test.lead")}</p>
      <div className="flex flex-wrap items-center gap-2">
        <label className="cursor-pointer text-body">
          <span className="rounded-md border border-dashed border-border px-3 py-2">{t("pipelines.test.drop")}</span>
          <input
            type="file"
            accept=".csv,.json,.txt,text/csv,application/json,text/plain"
            aria-label={t("pipelines.test.chooseFile")}
            className="sr-only"
            onChange={(event) => {
              void takeFile(event.target.files?.[0]);
              event.target.value = "";
            }}
          />
        </label>
        {sampleUrl ? (
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              setTrace(null);
              onTrace?.(null);
              setError(null);
              setSample({ name: sampleUrl, url: sampleUrl, format: "json" });
            }}
          >
            {t("pipelines.test.useUrl")}
          </Button>
        ) : null}
        {sample ? (
          <span className="font-mono text-caption text-fg-muted">
            {sample.url !== undefined
              ? t("pipelines.test.urlSample", { url: sample.url })
              : t("pipelines.test.sample", { name: sample.name, format: sample.format, bytes: sample.text.length })}
          </span>
        ) : null}
        <Button
          size="sm"
          variant="primary"
          disabled={!sample || !bloblang || running}
          onClick={() => {
            void run();
          }}
        >
          {running ? t("pipelines.test.running") : t("pipelines.test.run")}
        </Button>
        {sample && !bloblang ? <span className="text-caption text-fg-muted">{t("pipelines.test.noMapping")}</span> : null}
      </div>
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}
      {trace ? (
        <div className="flex flex-col gap-2" data-testid="stage-inspector">
          <div className="grid gap-2 md:grid-cols-3">
            <div className={panel}>
              <h4 className="text-caption font-semibold">{t("pipelines.test.input")}</h4>
              <p className="text-caption text-fg-muted">
                {t("pipelines.test.events", { events: trace.input.events, bytes: trace.input.bytes })}
              </p>
              {trace.input.sample !== undefined ? <pre className={pre}>{pretty(trace.input.sample)}</pre> : null}
            </div>
            <div className={panel}>
              <h4 className="text-caption font-semibold">{t("pipelines.test.mapping")}</h4>
              <p className="text-caption text-fg-muted">{t("pipelines.test.entities", { count: trace.mapping.length })}</p>
              {trace.mapping.length > 0 ? <pre className={pre}>{pretty(trace.mapping[0])}</pre> : null}
            </div>
            <div className={panel}>
              <h4 className="text-caption font-semibold">{t("pipelines.test.validation")}</h4>
              <ul className="text-caption">
                {trace.validation.map((v) => (
                  <li key={v.index} className={v.ok ? "text-success" : "text-danger"}>
                    #{v.index + 1} {v.ok ? t("pipelines.test.ok") : v.problems.join("; ")}
                  </li>
                ))}
              </ul>
            </div>
          </div>
          {trace.errors.length > 0 ? (
            <Alert tone="danger" title={t("pipelines.test.errors", { count: trace.errors.length })}>
              <ul className="text-caption">
                {trace.errors.map((e, i) => (
                  <li key={i}>
                    <span className="font-mono">{e.stage}</span>
                    {typeof e.line === "number" ? ` · ${t("pipelines.test.line", { line: e.line })}` : ""}: {e.message}
                  </li>
                ))}
              </ul>
            </Alert>
          ) : null}
          {lines.size > 0 && bloblang ? (
            <pre className={pre} aria-label={t("pipelines.field.bloblang")}>
              {bloblang.split("\n").map((line, i) => (
                <span
                  key={i}
                  data-line={i + 1}
                  className={lines.has(i + 1) ? "block bg-danger/10 underline decoration-danger decoration-wavy" : "block"}
                >
                  {String(i + 1).padStart(3, " ")} {line}
                </span>
              ))}
            </pre>
          ) : null}
          {ready ? <Alert tone="success">{t("pipelines.test.ready", { count: trace.mapping.length })}</Alert> : null}
        </div>
      ) : null}
    </section>
  );
}
