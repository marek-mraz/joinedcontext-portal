import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../api/client";
import {
  ACTIVITY_KINDS,
  ACTIVITY_SOURCES,
  activitySearch,
  mergeActivity,
  objectOf,
  subscribeActivity,
} from "../api/activity";
import type { ActivityEvent, ActivityQuery, Severity } from "../api/activity";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Field,
  Icon,
  Select,
  Switch,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "./ui";
import type { BadgeTone } from "./ui";

const COLUMNS = 4;

const SEVERITY_TONE: Record<string, BadgeTone> = {
  info: "neutral",
  warning: "warning",
  error: "danger",
};

/** One page of activity, its filters and its live tail (UI-31). */
export function ActivityFeed({
  project,
  fixed,
  compact,
  limit = 50,
}: {
  project: string;
  /** What this feed never leaves: an object page shows only its own object's events. */
  fixed?: ActivityQuery;
  /** An object page's panel: no filters, no tail toggle, just the last events. */
  compact?: boolean;
  limit?: number;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const ids = useId();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const [kind, setKind] = useState("");
  const [source, setSource] = useState("");
  const [severity, setSeverity] = useState("");
  const [tailing, setTailing] = useState(!compact);
  const [live, setLive] = useState<ActivityEvent[]>([]);
  const [arrived, setArrived] = useState(0);

  const query = useMemo<ActivityQuery>(
    () => ({
      ...fixed,
      kind: kind || fixed?.kind,
      source: source || fixed?.source,
      severity: (severity || fixed?.severity) as Severity | undefined,
      limit,
    }),
    [fixed, kind, source, severity, limit],
  );

  const page = useQuery({
    queryKey: ["projects", project, "activity", activitySearch(query)],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/activity", {
          params: { path: { project }, query: activitySearch(query) },
        }),
      ),
  });

  // A filter change starts the tail again: what arrived under the old filter is not this list.
  const filterKey = JSON.stringify(activitySearch(query));
  const lastFilter = useRef(filterKey);
  useEffect(() => {
    if (lastFilter.current !== filterKey) {
      lastFilter.current = filterKey;
      setLive([]);
      setArrived(0);
    }
  }, [filterKey]);

  useEffect(() => {
    if (!tailing) {
      return;
    }
    return subscribeActivity(project, query, (event) => {
      setLive((known) => {
        const next = mergeActivity(known, [event]);
        return next === known ? known : next;
      });
      setArrived((count) => count + 1);
    });
  }, [project, tailing, filterKey, query]);

  const items = useMemo(
    () => mergeActivity((page.data?.items ?? []) as ActivityEvent[], live).slice(0, limit),
    [page.data, live, limit],
  );

  const head = (
    <TableHead>
      <TableHeaderCell>{t("activity.time")}</TableHeaderCell>
      <TableHeaderCell>{t("activity.kind")}</TableHeaderCell>
      <TableHeaderCell>{t("activity.summary")}</TableHeaderCell>
      <TableHeaderCell>{t("activity.source")}</TableHeaderCell>
    </TableHead>
  );

  if (page.isPending) {
    return (
      <Table caption={t("activity.title")} status={t("app.loading")}>
        {head}
        <TableSkeleton columns={COLUMNS} />
      </Table>
    );
  }

  if (page.isError) {
    const message =
      page.error instanceof ApiError
        ? (page.error.problem?.detail ?? page.error.message)
        : t("app.error.generic");
    return (
      <Alert
        role="alert"
        tone="danger"
        actions={
          <Button
            size="sm"
            icon={<Icon name="refresh" className="size-4" />}
            onClick={() => {
              void page.refetch();
            }}
          >
            {t("app.error.retry")}
          </Button>
        }
      >
        {message}
      </Alert>
    );
  }

  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" });

  const filters = compact ? null : (
    <div className="flex flex-wrap items-end gap-3">
      <Field id={`${ids}-kind`} label={t("activity.filter.kind")} className="w-56">
        <Select
          id={`${ids}-kind`}
          value={kind}
          onChange={(event) => setKind(event.target.value)}
        >
          <option value="">{t("activity.filter.all")}</option>
          {ACTIVITY_KINDS.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </Select>
      </Field>
      <Field id={`${ids}-source`} label={t("activity.filter.source")} className="w-44">
        <Select
          id={`${ids}-source`}
          value={source}
          onChange={(event) => setSource(event.target.value)}
        >
          <option value="">{t("activity.filter.all")}</option>
          {ACTIVITY_SOURCES.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </Select>
      </Field>
      <Field id={`${ids}-severity`} label={t("activity.filter.severity")} className="w-44">
        <Select
          id={`${ids}-severity`}
          value={severity}
          onChange={(event) => setSeverity(event.target.value)}
        >
          <option value="">{t("activity.filter.all")}</option>
          {(["info", "warning", "error"] as const).map((option) => (
            <option key={option} value={option}>
              {t(`activity.severity.${option}`)}
            </option>
          ))}
        </Select>
      </Field>
      <Switch
        checked={tailing}
        onCheckedChange={setTailing}
        label={t("activity.tail")}
        aria-label={t("activity.tail")}
        className="mb-2"
      />
      <span className="mb-1.5 text-caption text-fg-subtle">
        {tailing ? t("activity.tailOn") : t("activity.tailOff")}
      </span>
    </div>
  );

  // Only an error interrupts a reader: the tail announces nothing else (a11y, UI-31).
  const criticals = live.filter((event) => event.severity === "error").length;

  return (
    <div className="flex flex-col gap-4">
      {filters}
      <p
        role="status"
        aria-live="polite"
        data-testid="activity-announce"
        className="sr-only"
      >
        {criticals > 0 ? t("activity.announce", { count: criticals }) : ""}
      </p>
      <span data-testid="activity-arrived" hidden>
        {arrived}
      </span>
      {items.length === 0 ? (
        <EmptyState
          icon="refresh"
          title={compact ? t("activity.panelEmpty") : t("activity.empty")}
        />
      ) : (
        <Table caption={t("activity.title")}>
          {head}
          <TableBody>
            {items.map((event) => {
              const object = objectOf(event);
              return (
                <TableRow key={`${event.time}-${event.kind}-${event.summary}`}>
                  <TableCell className="whitespace-nowrap text-fg-muted">
                    {time.format(new Date(event.time))}
                  </TableCell>
                  <TableCell>
                    <Badge tone={SEVERITY_TONE[event.severity] ?? "neutral"} mono>
                      {event.kind}
                    </Badge>
                  </TableCell>
                  <TableCell primary>
                    {event.summary}
                    {object ? (
                      <div className="mt-0.5">
                        <a
                          href={`/projects/${encodeURIComponent(project)}/${object}`}
                          className="focus-ring rounded-sm font-mono text-caption text-primary hover:underline"
                        >
                          {object}
                        </a>
                      </div>
                    ) : null}
                  </TableCell>
                  <TableCell className="text-fg-muted">{event.source}</TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      )}
    </div>
  );
}
