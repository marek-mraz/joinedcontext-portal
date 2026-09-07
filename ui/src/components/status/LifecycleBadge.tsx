import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";

export type LifecycleKind = "lane" | "phase" | "appLifecycle";

export interface LifecycleBadgeProps {
  kind: LifecycleKind;
  value?: string | null;
  className?: string;
}

const NEUTRAL = "bg-surface-subtle border-border";
const GOOD = "bg-emerald-500/15 border-emerald-500/40";
const WARN = "bg-amber-500/20 border-amber-500/40";
const BAD = "bg-danger/15 border-danger/40";

/** Lower-cased status value → the label key under `lane.`/`phase.` and its chip colour. */
const LANES: Record<string, [string, string]> = {
  green: ["green", GOOD],
  yellow: ["yellow", WARN],
  red: ["red", BAD],
};

const PHASES: Record<string, [string, string]> = {
  draft: ["draft", NEUTRAL],
  pending: ["pendingApproval", WARN],
  pendingapproval: ["pendingApproval", WARN],
  deploying: ["deploying", "bg-blue-500/15 border-blue-500/40"],
  live: ["live", GOOD],
  merged: ["merged", GOOD],
  applied: ["applied", GOOD],
  error: ["error", BAD],
  rejected: ["rejected", BAD],
  drifted: ["drifted", "bg-purple-500/15 border-purple-500/40"],
  // What a `SyncSource` reports about its own loop (MF-30). `pendingapproval` and `error`
  // above mean the same thing for it, so only the three it adds are here.
  synced: ["synced", GOOD],
  outofsync: ["outOfSync", WARN],
  paused: ["paused", NEUTRAL],
};

/** An app's own lifecycle, which is not the reconciler's phase (AP-18). */
const APP_LIFECYCLE: Record<string, [string, string]> = {
  draft: ["draft", NEUTRAL],
  preview: ["preview", WARN],
  published: ["published", GOOD],
  retired: ["retired", NEUTRAL],
};

/**
 * One chip for both halves of a resource's lifecycle: the risk lane a change falls into
 * and the phase the reconciler has it in. The tooltip carries the explanation, so the
 * colour is never the only carrier of meaning.
 */
export function LifecycleBadge({ kind, value, className }: LifecycleBadgeProps): JSX.Element {
  const { t } = useTranslation();
  const raw = value ?? "";
  const vocabulary =
    kind === "lane" ? LANES : kind === "appLifecycle" ? APP_LIFECYCLE : PHASES;
  const entry = vocabulary[raw.toLowerCase()];

  return (
    <span
      title={entry ? t(`${kind}.${entry[0]}Help`) : undefined}
      className={clsx(
        "inline-flex items-center rounded border px-2 py-0.5 text-xs font-medium text-surface-fg",
        entry ? entry[1] : NEUTRAL,
        className,
      )}
    >
      {entry ? t(`${kind}.${entry[0]}`) : raw}
    </span>
  );
}
