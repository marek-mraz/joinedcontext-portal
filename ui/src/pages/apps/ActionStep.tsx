import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { RunEvent } from "./useAgentRun";

/**
 * Why a step failed, on one line, or "" when the step says nothing: a plain error, a runtime
 * error with the file and line it was thrown at, or the blocks a patch could not apply. The
 * transcript shows it on the step's own row, so a failure is readable without opening it.
 */
export function errorText(
  payload: Record<string, unknown>,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const { error, refused } = payload;
  if (typeof error === "string") {
    return error;
  }
  if (typeof error === "object" && error !== null) {
    const { message, file, line } = error as Record<string, unknown>;
    if (typeof message === "string" && message !== "") {
      return typeof file === "string" && file !== "" && line !== undefined && line !== null
        ? `${message} (${file}:${String(line)})`
        : message;
    }
  }
  if (Array.isArray(refused) && refused.length > 0) {
    const first = refused[0] as Record<string, unknown> | null;
    const reason = typeof first?.reason === "string" ? first.reason : "";
    const path = typeof first?.path === "string" && first.path !== "" ? first.path : "";
    return t("agentRun.step.refusedLine", {
      count: refused.length,
      reason: path !== "" && reason !== "" ? `${path}: ${reason}` : reason || path,
    });
  }
  return "";
}

/**
 * One step the assistant took, inspectable (AG-56, OPS-50).
 *
 * The summary is the line the transcript always showed; open, the step shows what the tool was
 * given, what it produced or why it failed, and the change it made. Every value comes from the
 * workspace and is rendered as text, never as markup (AG-46): a `<script>` in an output is a
 * `<script>` on the screen and nothing else. A failed step can be sent back into the run with
 * one click, and every step hands its trace over as JSON.
 */
export function ActionStep({
  event,
  live,
  onSend,
  count = 1,
}: {
  event: RunEvent;
  live: boolean;
  onSend: (text: string) => void;
  /** How many times in a row the same step repeated; the event is the newest of them. */
  count?: number;
}): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const payload = event.payload;
  const tool = typeof payload.tool === "string" && payload.tool !== "" ? payload.tool : "tool";
  const exitCode = typeof payload.exitCode === "number" ? payload.exitCode : undefined;
  const failed =
    payload.status === "failed" ||
    payload.error !== undefined ||
    (exitCode !== undefined && exitCode !== 0);
  const duration = typeof payload.durationMs === "number" ? payload.durationMs : undefined;
  const reason = failed ? errorText(payload, t) : "";
  const refused = Array.isArray(payload.refused) && payload.refused.length > 0 ? payload.refused : undefined;
  const sections: Array<[string, unknown]> = [
    [t("agentRun.step.input"), payload.input ?? payload.command],
    [t("agentRun.step.output"), payload.output],
    [t("agentRun.step.error"), payload.error],
    [t("agentRun.step.refused"), refused],
    [t("agentRun.step.diff"), payload.diff],
  ];

  const copy = async (): Promise<void> => {
    const trace = {
      seq: event.seq,
      tool,
      status: failed ? "failed" : "ok",
      durationMs: duration,
      input: payload.input ?? payload.command,
      output: payload.output,
      error: payload.error,
      diff: payload.diff,
    };
    try {
      await navigator.clipboard.writeText(JSON.stringify(trace, null, 2));
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <details className="rounded border border-border bg-surface-subtle text-xs">
      <summary className="flex cursor-pointer items-center gap-2 px-2 py-1 font-mono">
        <span
          role="img"
          aria-label={failed ? t("agentRun.step.failed") : t("agentRun.step.ok")}
          className={failed ? "text-danger" : "text-success"}
        >
          {failed ? "✗" : "✓"}
        </span>
        <span className="min-w-0 break-words">{tool}</span>
        {count > 1 ? (
          <span data-testid="step-count" className="rounded bg-surface px-1 text-fg-muted">
            ×{count}
          </span>
        ) : null}
        {duration !== undefined ? (
          <span className="shrink-0 text-fg-muted">{t("agentRun.step.duration", { ms: duration })}</span>
        ) : null}
        {reason !== "" ? (
          <span data-testid="step-reason" title={reason} className="min-w-0 truncate text-danger">
            {reason}
          </span>
        ) : null}
      </summary>
      <div className="space-y-2 px-2 pb-2">
        {sections.map(([label, value]) =>
          value === undefined || value === null || value === "" ? null : (
            <div key={label}>
              <p className="font-medium text-fg-muted">{label}</p>
              <pre className="mt-0.5 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded bg-surface p-2 font-mono">
                {typeof value === "string" ? value : JSON.stringify(value, null, 2)}
              </pre>
            </div>
          ),
        )}
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() => {
              void copy();
            }}
            className="rounded border border-border bg-surface px-2 py-0.5 hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {copied ? t("agentRun.step.copied") : t("agentRun.step.copy")}
          </button>
          {failed && live ? (
            <button
              type="button"
              onClick={() => {
                onSend(
                  t("agentRun.step.fixMessage", {
                    tool,
                    error: reason !== "" ? reason : String(exitCode ?? ""),
                  }),
                );
              }}
              className="rounded border border-border bg-surface px-2 py-0.5 hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {t("agentRun.step.fix")}
            </button>
          ) : null}
        </div>
      </div>
    </details>
  );
}
