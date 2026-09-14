import type { ReactNode } from "react";
import { useEffect, useMemo, useRef } from "react";
import * as echarts from "echarts";
import { aggregate, currentTokens, echartsTheme, groupBy } from "@joinedcontext/sdk";
import type { Agg, DesignTokens, ProblemError, Row, TemporalRow } from "@joinedcontext/sdk";
import { Empty, Loading, Problem } from "./states";

export interface ChartSpec {
  x: string;
  y?: string;
  agg?: Agg;
  top?: number;
}

export type Bucket = "hour" | "day" | "week" | "month";

export function bucketOf(iso: string, bucket: Bucket): string | null {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;

  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const dt = String(d.getUTCDate()).padStart(2, "0");
  const h = String(d.getUTCHours()).padStart(2, "0");

  switch (bucket) {
    case "hour":
      return `${y}-${m}-${dt}T${h}:00:00.000Z`;
    case "day":
      return `${y}-${m}-${dt}T00:00:00.000Z`;
    case "month":
      return `${y}-${m}-01T00:00:00.000Z`;
    case "week": {
      const day = d.getUTCDay();
      const diff = (day === 0 ? -6 : 1) - day;
      const monday = new Date(Date.UTC(y, d.getUTCMonth(), d.getUTCDate() + diff, 0, 0, 0, 0));
      return monday.toISOString();
    }
  }
}

function hasSeriesData(option: Record<string, unknown>): boolean {
  const series = option.series as Array<{ data?: unknown[] }> | undefined;
  if (!Array.isArray(series) || series.length === 0) return false;
  return series.some((s) => Array.isArray(s.data) && s.data.length > 0);
}

export function barOption(
  rows: Row[],
  spec: ChartSpec & { horizontal?: boolean },
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const agg = spec.agg ?? (spec.y ? "sum" : "count");
  const groups = groupBy(rows, spec.x, agg, spec.y, spec.top ?? 20);

  const categoryAxis = { type: "category", data: groups.map((g) => g.key) };
  const valueAxis = { type: "value" };
  const series = [{ type: "bar", name: spec.y ?? "count", data: groups.map((g) => g.value) }];

  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
    xAxis: spec.horizontal ? valueAxis : categoryAxis,
    yAxis: spec.horizontal ? categoryAxis : valueAxis,
    series,
  };
}

export function lineOption(
  rows: Row[],
  spec: ChartSpec,
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const agg = spec.agg ?? (spec.y ? "sum" : "count");
  const groups = groupBy(rows, spec.x, agg, spec.y);

  const allNumeric =
    groups.length > 0 &&
    groups.every((g) => g.key.trim() !== "" && !Number.isNaN(Number(g.key)));

  const sorted = [...groups].sort((a, b) => {
    if (allNumeric) {
      return Number(a.key) - Number(b.key);
    }
    return a.key.localeCompare(b.key);
  });

  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
    xAxis: { type: "category", data: sorted.map((g) => g.key) },
    yAxis: { type: "value" },
    series: [{ type: "line", showSymbol: false, data: sorted.map((g) => g.value) }],
  };
}

export function pieOption(
  rows: Row[],
  spec: ChartSpec,
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const agg = spec.agg ?? (spec.y ? "sum" : "count");
  const groups = groupBy(rows, spec.x, agg, spec.y);

  const limit = spec.top ?? 8;
  let data: Array<{ name: string; value: number }>;

  if (groups.length <= limit) {
    data = groups.map((g) => ({ name: g.key, value: g.value }));
  } else {
    const topGroups = groups.slice(0, limit);
    const rest = groups.slice(limit);
    const otherVal = rest.reduce((acc, g) => acc + g.value, 0);
    data = [...topGroups.map((g) => ({ name: g.key, value: g.value })), { name: "Other", value: otherVal }];
  }

  return {
    color: t.chart.palette,
    tooltip: { trigger: "item" },
    series: [{ type: "pie", radius: ["40%", "70%"], data }],
  };
}

export function timeSeriesOption(
  input:
    | { rows: Row[]; time: string; y?: string; agg?: Agg; bucket?: Bucket }
    | { series: TemporalRow[]; attr: string },
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();

  if ("rows" in input) {
    const { rows, time, y, agg = y ? "avg" : "count", bucket = "day" } = input;
    const buckets = new Map<string, Row[]>();
    for (const row of rows) {
      const rawTime = row[time];
      if (typeof rawTime !== "string") continue;
      const b = bucketOf(rawTime, bucket);
      if (!b) continue;
      const existing = buckets.get(b);
      if (existing) existing.push(row);
      else buckets.set(b, [row]);
    }
    const points: Array<[string, number]> = [];
    for (const [b, bucketRows] of buckets.entries()) {
      const val = aggregate(bucketRows, agg, y);
      if (val !== null) {
        points.push([b, val]);
      }
    }
    points.sort((a, b) => a[0].localeCompare(b[0]));

    return {
      color: t.chart.palette,
      tooltip: { trigger: "axis" },
      grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
      xAxis: { type: "time" },
      yAxis: { type: "value" },
      series: [{ type: "line", data: points }],
    };
  }

  const { series, attr } = input;
  const lineSeries = series.map((s) => {
    const pts = (s.series[attr] ?? [])
      .filter((p) => typeof p.value === "number")
      .map((p) => [p.observedAt, p.value as number] as [string, number])
      .sort((a, b) => a[0].localeCompare(b[0]));
    return {
      type: "line",
      name: s.id,
      data: pts,
    };
  });

  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
    xAxis: { type: "time" },
    yAxis: { type: "value" },
    series: lineSeries,
  };
}

export function ChartCard({
  title,
  option,
  height,
  loading,
  error,
  empty,
  onSelect,
  onReady,
}: {
  title?: string;
  option: Record<string, unknown> | null;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
  empty?: ReactNode;
  onSelect?: (name: string) => void;
  onReady?: (chart: echarts.ECharts) => void;
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const onReadyRef = useRef(onReady);
  onReadyRef.current = onReady;

  const showCanvas = !error && !loading && option !== null;

  useEffect(() => {
    if (!showCanvas) return;
    const el = containerRef.current;
    if (!el) return;

    const chart = echarts.init(el, echartsTheme(currentTokens()), { renderer: "canvas" });
    chartRef.current = chart;
    onReadyRef.current?.(chart);

    chart.on("click", (p: unknown) => {
      const param = p as { name?: unknown };
      if (param && param.name !== undefined) {
        onSelectRef.current?.(String(param.name));
      }
    });

    if (option) {
      chart.setOption(option, true);
    }

    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(() => {
        chart.resize();
      });
      observer.observe(el);
    }

    return () => {
      observer?.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, [showCanvas]);

  useEffect(() => {
    if (chartRef.current && option) {
      chartRef.current.setOption(option, true);
    }
  }, [option]);

  return (
    <figure className="jc-chart">
      {title && <figcaption>{title}</figcaption>}
      {error ? (
        <Problem error={error} />
      ) : loading ? (
        <Loading />
      ) : option === null ? (
        <Empty>{empty ?? "Nothing to chart for the current filters."}</Empty>
      ) : (
        <div
          className="jc-chart-canvas"
          style={{ height: height ?? 280 }}
          ref={containerRef}
        />
      )}
    </figure>
  );
}

export function BarChartCard({
  rows,
  x,
  y,
  agg,
  top,
  horizontal,
  title,
  height,
  loading,
  error,
  onSelect,
}: ChartSpec & {
  rows: Row[];
  title?: string;
  horizontal?: boolean;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
  onSelect?: (name: string) => void;
}): React.JSX.Element {
  const option = useMemo(() => {
    const opt = barOption(rows, { x, y, agg, top, horizontal });
    return hasSeriesData(opt) ? opt : null;
  }, [rows, x, y, agg, top, horizontal]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
      onSelect={onSelect}
    />
  );
}

export function LineChartCard({
  rows,
  x,
  y,
  agg,
  top,
  title,
  height,
  loading,
  error,
}: ChartSpec & {
  rows: Row[];
  title?: string;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
}): React.JSX.Element {
  const option = useMemo(() => {
    const opt = lineOption(rows, { x, y, agg, top });
    return hasSeriesData(opt) ? opt : null;
  }, [rows, x, y, agg, top]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
    />
  );
}

export function PieChartCard({
  rows,
  x,
  y,
  agg,
  top,
  title,
  height,
  loading,
  error,
  onSelect,
}: ChartSpec & {
  rows: Row[];
  title?: string;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
  onSelect?: (name: string) => void;
}): React.JSX.Element {
  const option = useMemo(() => {
    const opt = pieOption(rows, { x, y, agg, top });
    return hasSeriesData(opt) ? opt : null;
  }, [rows, x, y, agg, top]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
      onSelect={onSelect}
    />
  );
}

export function TimeSeriesCard(
  props: ({ rows: Row[]; time: string; y?: string; agg?: Agg; bucket?: Bucket } | { series: TemporalRow[]; attr: string }) & {
    title?: string;
    height?: number;
    loading?: boolean;
    error?: ProblemError | Error | null;
  },
): React.JSX.Element {
  const { title, height, loading, error } = props;
  const option = useMemo(() => {
    const opt = timeSeriesOption(props);
    return hasSeriesData(opt) ? opt : null;
  }, [props]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
    />
  );
}
