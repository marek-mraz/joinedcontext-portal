import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Badge } from "../../components/ui/Badge";
import type { BadgeTone } from "../../components/ui/Badge";
import { buttonClass } from "../../components/ui/Button";

/**
 * What the assistant found, as cards (UI-46, AG-58): one per item of a `search_catalog` tool
 * step, with the kind, the owner, how fresh the feed is, whether the door is open to the person
 * and why, and the two things to do next. Every string comes from the run's stream and is
 * rendered as text, never as markup (AG-46).
 */

export interface CatalogItem {
  kind: string;
  name: string;
  space: string;
  owner: string;
  title?: string;
  endpointSlug?: string;
  matchReason: string[];
  access: { verdict: string; reason: string };
  freshness: { pipeline: string; scrapedAt: string; received?: number; errors?: number } | null;
}

const KIND_TONE: Record<string, BadgeTone> = {
  Endpoint: "primary",
  ContextSpace: "info",
  DataModel: "purple",
};

const KIND_PLURAL: Record<string, string> = {
  Endpoint: "endpoints",
  ContextSpace: "spaces",
  DataModel: "datamodels",
};

/** The items of a `search_catalog` step, or none when the payload is not what the Portal wrote. */
export function catalogItemsOf(output: unknown): CatalogItem[] {
  if (typeof output !== "object" || output === null || !Array.isArray((output as { items?: unknown }).items)) {
    return [];
  }
  return ((output as { items: unknown[] }).items as unknown[]).filter(isItem);
}

function isItem(value: unknown): value is CatalogItem {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const item = value as Record<string, unknown>;
  const access = item.access as Record<string, unknown> | undefined;
  return (
    typeof item.kind === "string" &&
    typeof item.name === "string" &&
    typeof item.space === "string" &&
    typeof access === "object" &&
    access !== null &&
    typeof access.verdict === "string"
  );
}

/** "12 s ago", "4 min ago", "3 h ago", "2 d ago" from an RFC 3339 instant; the raw text when it is not one. */
export function ago(iso: string, now: number, t: (key: string, values?: Record<string, unknown>) => string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) {
    return iso;
  }
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) {
    return t("agentRun.catalog.agoSeconds", { n: seconds });
  }
  if (seconds < 3600) {
    return t("agentRun.catalog.agoMinutes", { n: Math.round(seconds / 60) });
  }
  if (seconds < 86400) {
    return t("agentRun.catalog.agoHours", { n: Math.round(seconds / 3600) });
  }
  return t("agentRun.catalog.agoDays", { n: Math.round(seconds / 86400) });
}

export function CatalogCards({
  project,
  items,
  now,
}: {
  project: string;
  items: CatalogItem[];
  /** The instant freshness is counted from; the mount time unless a test fixes it. */
  now?: number;
}): JSX.Element {
  const { t } = useTranslation();
  const [mounted] = useState(() => Date.now());
  const at = now ?? mounted;
  if (items.length === 0) {
    return <p className="text-sm text-fg-muted">{t("agentRun.catalog.none")}</p>;
  }
  return (
    <ul className="grid gap-2 sm:grid-cols-2" aria-label={t("agentRun.catalog.title")}>
      {items.map((item) => {
        const open = item.access.verdict === "allowed";
        return (
          <li
            key={`${item.kind}/${item.name}`}
            className="flex flex-col gap-2 rounded-lg border border-border bg-surface p-3 text-sm"
          >
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone={KIND_TONE[item.kind] ?? "neutral"}>{item.kind}</Badge>
              <span className="min-w-0 break-words font-medium">{item.title ?? item.name}</span>
            </div>
            <p className="font-mono text-xs text-fg-muted">
              {item.name} · {t("agentRun.catalog.owner", { owner: item.owner })}
            </p>
            <div className="flex flex-wrap gap-2">
              <Badge tone={open ? "success" : "warning"} title={item.access.reason}>
                {open ? t("agentRun.catalog.allowed") : t("agentRun.catalog.restricted")}
                <span className="font-normal">· {item.access.reason}</span>
              </Badge>
              <Badge tone={item.freshness ? "info" : "neutral"}>
                {item.freshness
                  ? t("agentRun.catalog.fresh", {
                      when: ago(item.freshness.scrapedAt, at, t),
                      pipeline: item.freshness.pipeline,
                    })
                  : t("agentRun.catalog.noFeed")}
              </Badge>
            </div>
            <div className="flex flex-wrap gap-2">
              {open && item.kind !== "DataModel" ? (
                <Link
                  to="/projects/$project/explore"
                  params={{ project }}
                  search={{
                    space: item.space,
                    endpoint: item.kind === "Endpoint" ? item.name : undefined,
                  }}
                  className={buttonClass("primary", "sm")}
                >
                  {t("agentRun.catalog.explore")}
                </Link>
              ) : null}
              <Link
                to="/projects/$project/$plural"
                params={{ project, plural: KIND_PLURAL[item.kind] ?? "endpoints" }}
                className={buttonClass("secondary", "sm")}
              >
                {t("agentRun.catalog.open")}
              </Link>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
