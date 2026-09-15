import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { Icon } from "../../components/ui/icons";
import type { IconName } from "../../components/ui/icons";

/**
 * What the assistant found, as a compact list (UI-46, AG-58): one row per item of a
 * `search_catalog` tool step, with the kind, the title, the name, whether the door is open to the
 * person (a dot, the reason on hover) and how fresh the feed is when it is known. Four rows show;
 * the rest wait behind "Show more". An endpoint row can be put into the conversation's data bar
 * (AG-75). Every string comes from the run's stream and is rendered as text, never as markup
 * (AG-46).
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

const KIND_ICON: Record<string, IconName> = {
  Endpoint: "endpoints",
  ContextSpace: "spaces",
  DataModel: "models",
};

/** Rows shown before "Show more". */
const FIRST_ROWS = 4;

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
  onUseEndpoint,
  usedEndpoints = [],
  words = "",
}: {
  project: string;
  items: CatalogItem[];
  /** The instant freshness is counted from; the mount time unless a test fixes it. */
  now?: number;
  /** Puts an endpoint into the conversation's data bar; without it rows offer no "Use". */
  onUseEndpoint?: (name: string) => void;
  /** The endpoints the conversation already reads, whose "Use" is spent. */
  usedEndpoints?: string[];
  /** The words the search ran on, named when nothing matched them. */
  words?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const [mounted] = useState(() => Date.now());
  const [expanded, setExpanded] = useState(false);
  const at = now ?? mounted;
  if (items.length === 0) {
    return (
      <p className="text-sm text-fg-muted">
        {words.trim() === "" ? t("agentRun.catalog.none") : t("agentRun.catalog.noneFor", { words: words.trim() })}
      </p>
    );
  }
  const shown = expanded ? items : items.slice(0, FIRST_ROWS);
  const hidden = items.length - FIRST_ROWS;
  return (
    <div className="flex min-w-0 max-w-full flex-col text-xs">
      <ul
        className="min-w-0 divide-y divide-border rounded-lg border border-border bg-surface"
        aria-label={t("agentRun.catalog.title")}
      >
        {shown.map((item) => {
          const open = item.access.verdict === "allowed";
          const access = `${open ? t("agentRun.catalog.allowed") : t("agentRun.catalog.restricted")} · ${item.access.reason}`;
          const inUse = usedEndpoints.includes(item.name);
          const linkClass =
            "min-w-0 truncate font-medium text-fg hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus";
          return (
            <li key={`${item.kind}/${item.name}`} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 px-2 py-1.5">
              <Icon name={KIND_ICON[item.kind] ?? "endpoints"} title={item.kind} className="size-3.5 text-fg-muted" />
              <span className="flex min-w-0 flex-1 basis-40 items-baseline gap-1.5">
                {open && item.kind !== "DataModel" ? (
                  <Link
                    to="/projects/$project/explore"
                    params={{ project }}
                    search={{
                      space: item.space,
                      endpoint: item.kind === "Endpoint" ? item.name : undefined,
                    }}
                    title={item.title ?? item.name}
                    className={linkClass}
                  >
                    {item.title ?? item.name}
                  </Link>
                ) : (
                  <Link
                    to="/projects/$project/$plural"
                    params={{ project, plural: KIND_PLURAL[item.kind] ?? "endpoints" }}
                    title={item.title ?? item.name}
                    className={linkClass}
                  >
                    {item.title ?? item.name}
                  </Link>
                )}
                {item.title && item.title !== item.name ? (
                  <span title={item.name} className="min-w-0 truncate font-mono text-[11px] text-fg-muted">
                    {item.name}
                  </span>
                ) : null}
              </span>
              {item.freshness ? (
                <span
                  className="shrink-0 text-fg-muted"
                  title={t("agentRun.catalog.fresh", {
                    when: ago(item.freshness.scrapedAt, at, t),
                    pipeline: item.freshness.pipeline,
                  })}
                >
                  {t("agentRun.catalog.fed", { when: ago(item.freshness.scrapedAt, at, t) })}
                </span>
              ) : null}
              <span
                role="img"
                aria-label={access}
                title={access}
                className={clsx("size-2 shrink-0 rounded-full", open ? "bg-success" : "bg-fg-muted opacity-50")}
              />
              {onUseEndpoint && item.kind === "Endpoint" && open ? (
                <button
                  type="button"
                  disabled={inUse}
                  aria-label={
                    inUse
                      ? t("agentRun.catalog.inUseLabel", { name: item.name })
                      : t("agentRun.catalog.useLabel", { name: item.name })
                  }
                  onClick={() => {
                    onUseEndpoint(item.name);
                  }}
                  className="shrink-0 rounded-full border border-border px-2 py-0.5 text-[11px] font-medium text-fg hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:cursor-default disabled:border-transparent disabled:text-fg-muted disabled:hover:bg-transparent"
                >
                  {inUse ? t("agentRun.catalog.inUse") : t("agentRun.catalog.use")}
                </button>
              ) : null}
            </li>
          );
        })}
      </ul>
      {hidden > 0 ? (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => {
            setExpanded((was) => !was);
          }}
          className="self-start rounded px-1 py-0.5 text-fg-muted hover:text-fg focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {expanded ? t("agentRun.catalog.showLess") : t("agentRun.catalog.showMore", { count: hidden })}
        </button>
      ) : null}
    </div>
  );
}
