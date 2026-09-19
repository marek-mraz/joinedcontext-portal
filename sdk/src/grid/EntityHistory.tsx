/**
 * How one value got to where it is (T-1431; UI-66, EP-07): the temporal answer of one attribute of
 * one entity, as a table and — for numbers — one line drawn from it.
 *
 * The read goes through the same source the grid reads, so it is the person's own session and the
 * Endpoint's Policy that decide it, temporal clamping and projection included. The series is
 * bounded by `maxPoints` before it is asked for, because one click must not ask a 16 GB node for a
 * year of a stream, and the panel says when the window was cut rather than showing a short series
 * as if it were the whole one.
 *
 * `ponytail: SVG polyline, a chart library when more than one series is wanted.`
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import type { EntitySource, HistoryPoint, HistoryWindow } from "./source";

/** The windows a person picks from, as hours back from now; `custom` asks for two timestamps. */
export const WINDOWS = ["hour", "day", "week", "custom"] as const;
export type Window = (typeof WINDOWS)[number];

const HOURS: Record<Exclude<Window, "custom">, number> = { hour: 1, day: 24, week: 24 * 7 };

/** The most points one read may ask for, whatever the window (EP-07). */
export const MAX_POINTS = 1000;

export interface HistoryLabels {
  title: string;
  window: Record<Window, string>;
  from: string;
  to: string;
  at: string;
  value: string;
  unit: string;
  empty: string;
  cut: string;
  copy: string;
  close: string;
  loading: string;
  error: string;
}

export const DEFAULT_HISTORY_LABELS: HistoryLabels = {
  title: "History",
  window: { hour: "Last hour", day: "Last day", week: "Last week", custom: "Between" },
  from: "From",
  to: "To",
  at: "Observed",
  value: "Value",
  unit: "Unit",
  empty: "Nothing was recorded in this window.",
  cut: "the oldest points are not shown",
  copy: "Copy as CSV",
  close: "Close",
  loading: "Loading…",
  error: "Error",
};

/** The window a choice asks for, in UTC, because a broker compares timestamps and not clocks. */
export function windowOf(
  choice: Window,
  custom: { from: string; to: string },
  maxPoints: number,
  now: () => Date = () => new Date(),
): HistoryWindow {
  if (choice === "custom") {
    return {
      from: custom.from ? new Date(custom.from).toISOString() : undefined,
      to: custom.to ? new Date(custom.to).toISOString() : undefined,
      lastN: maxPoints,
    };
  }
  const end = now();
  const start = new Date(end.getTime() - HOURS[choice] * 3600 * 1000);
  return { from: start.toISOString(), to: end.toISOString(), lastN: maxPoints };
}

/** The series as CSV, for a spreadsheet: the two columns a person came for, quoted. */
export function asCsv(points: HistoryPoint[], attr: string): string {
  const cell = (value: unknown): string => {
    const text = value === null || value === undefined ? "" : String(value);
    return /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  };
  return [`observedAt,${cell(attr)}`, ...points.map((point) => `${cell(point.at)},${cell(point.value)}`)].join("\n");
}

/** The polyline of a numeric series in a 0…100 box, or `null` when there is nothing to draw. */
export function polyline(points: HistoryPoint[]): string | null {
  const numbers = points
    .map((point) => ({ at: Date.parse(point.at), value: Number(point.value) }))
    .filter((point) => Number.isFinite(point.at) && Number.isFinite(point.value));
  if (numbers.length < 2) {
    return null;
  }
  const times = numbers.map((point) => point.at);
  const values = numbers.map((point) => point.value);
  const minAt = Math.min(...times);
  const maxAt = Math.max(...times);
  const minValue = Math.min(...values);
  const maxValue = Math.max(...values);
  const spanAt = maxAt - minAt || 1;
  const spanValue = maxValue - minValue || 1;
  return numbers
    .map((point) => {
      const x = ((point.at - minAt) / spanAt) * 100;
      // SVG's y grows downward, so the largest value sits at the top where a person expects it.
      const y = 100 - ((point.value - minValue) / spanValue) * 100;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

export interface EntityHistoryProps {
  source: Pick<EntitySource, "history">;
  id: string;
  attr: string;
  /** What the value is measured in, as the cell showed it. */
  unit?: string;
  maxPoints?: number;
  labels?: Partial<HistoryLabels>;
  onClose?: () => void;
  now?: () => Date;
}

export function EntityHistory(props: EntityHistoryProps): React.JSX.Element {
  const { source, id, attr, unit, onClose, now } = props;
  const maxPoints = Math.min(props.maxPoints ?? MAX_POINTS, MAX_POINTS);
  const labels = useMemo(
    (): HistoryLabels => ({
      ...DEFAULT_HISTORY_LABELS,
      ...props.labels,
      window: { ...DEFAULT_HISTORY_LABELS.window, ...props.labels?.window },
    }),
    [props.labels],
  );

  const [choice, setChoice] = useState<Window>("day");
  const [custom, setCustom] = useState({ from: "", to: "" });
  const [points, setPoints] = useState<HistoryPoint[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const read = useCallback(async () => {
    if (!source.history) {
      return;
    }
    setError(null);
    setPoints(null);
    try {
      const got = await source.history(id, attr, windowOf(choice, custom, maxPoints, now));
      setPoints(got);
    } catch (err) {
      setError(err instanceof Error && err.message ? err.message : labels.error);
    }
  }, [source, id, attr, choice, custom, maxPoints, now, labels.error]);

  useEffect(() => {
    void read();
  }, [read]);

  const line = points ? polyline(points) : null;

  return (
    <section className="jc-grid-history" aria-label={`${labels.title} ${attr}`}>
      <header>
        {/* The entity as well as the attribute: the panel is opened from a column, so which row
            it is about has to be readable in it. */}
        <h3>{`${labels.title}: ${attr} — ${id}`}</h3>
        {onClose && (
          <button type="button" onClick={onClose}>
            {labels.close}
          </button>
        )}
      </header>

      <fieldset>
        {WINDOWS.map((each) => (
          <label key={each}>
            <input
              type="radio"
              name={`jc-history-${attr}`}
              checked={choice === each}
              onChange={() => setChoice(each)}
            />
            {labels.window[each]}
          </label>
        ))}
      </fieldset>

      {choice === "custom" && (
        <fieldset>
          <label>
            {labels.from}
            <input
              type="datetime-local"
              value={custom.from}
              onChange={(e) => setCustom({ ...custom, from: e.target.value })}
            />
          </label>
          <label>
            {labels.to}
            <input
              type="datetime-local"
              value={custom.to}
              onChange={(e) => setCustom({ ...custom, to: e.target.value })}
            />
          </label>
        </fieldset>
      )}

      {error && <p className="jc-grid-error">{`${labels.error}: ${error}`}</p>}
      {!error && points === null && <p>{labels.loading}</p>}
      {points !== null && points.length === 0 && <p>{labels.empty}</p>}

      {points !== null && points.length > 0 && (
        <>
          {points.length >= maxPoints && <p className="jc-grid-history-cut">{labels.cut}</p>}
          {line && (
            <svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label={`${labels.title} ${attr}`}>
              <polyline points={line} fill="none" stroke="currentColor" strokeWidth="1" vectorEffect="non-scaling-stroke" />
            </svg>
          )}
          <table>
            <thead>
              <tr>
                <th>{labels.at}</th>
                <th>{labels.value}</th>
                {unit && <th>{labels.unit}</th>}
              </tr>
            </thead>
            <tbody>
              {points.map((point, index) => (
                <tr key={`${point.at}-${index}`}>
                  <td>{point.at}</td>
                  <td>{point.value === null || point.value === undefined ? "" : String(point.value)}</td>
                  {unit && <td>{unit}</td>}
                </tr>
              ))}
            </tbody>
          </table>
          <button type="button" onClick={() => void navigator.clipboard?.writeText(asCsv(points, attr))}>
            {labels.copy}
          </button>
        </>
      )}
    </section>
  );
}
