import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { currentTokens } from "@joinedcontext/sdk";
import type { Row, TemporalRow } from "@joinedcontext/sdk";
import {
  BarChartCard,
  barOption,
  bucketOf,
  ChartCard,
  lineOption,
  pieOption,
  timeSeriesOption,
} from "./charts";

let clickCallback: ((params: { name: string }) => void) | undefined;
const mockChartInstance = {
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
  on: vi.fn((event: string, cb: (params: { name: string }) => void) => {
    if (event === "click") {
      clickCallback = cb;
    }
  }),
};

vi.mock("echarts", () => ({
  init: vi.fn(() => mockChartInstance),
}));

describe("charts options and utilities", () => {
  const sampleRows: Row[] = [
    { id: "1", type: "T", station: "Kaivopuisto", bikes: 10, ts: "2025-05-14T12:30:00Z" },
    { id: "2", type: "T", station: "Kaivopuisto", bikes: 20, ts: "2025-05-14T12:45:00Z" },
    { id: "3", type: "T", station: "Kamppi", bikes: 5, ts: "2025-05-14T14:15:00Z" },
  ];

  it("barOption creates top-N order and count without y", () => {
    const opt = barOption(sampleRows, { x: "station" });
    const series = (opt.series as Array<{ data: number[] }>)[0];
    const xAxis = opt.xAxis as { data: string[] };

    expect(xAxis.data).toEqual(["Kaivopuisto", "Kamppi"]);
    expect(series.data).toEqual([2, 1]);
    expect(opt.color).toEqual(currentTokens().chart.palette);
  });

  it("barOption horizontal swaps axes", () => {
    const opt = barOption(sampleRows, { x: "station", horizontal: true });
    expect(opt.xAxis).toEqual({ type: "value" });
    expect(opt.yAxis).toMatchObject({ type: "category", data: ["Kaivopuisto", "Kamppi"] });
  });

  it("lineOption sorts numeric keys ascending", () => {
    const numRows: Row[] = [
      { id: "1", type: "T", year: "2024", count: 100 },
      { id: "2", type: "T", year: "2020", count: 50 },
      { id: "3", type: "T", year: "2022", count: 80 },
    ];
    const opt = lineOption(numRows, { x: "year", y: "count" });
    const xAxis = opt.xAxis as { data: string[] };
    expect(xAxis.data).toEqual(["2020", "2022", "2024"]);
  });

  it("pieOption produces Other slice only when rest exceeds top", () => {
    const manyRows: Row[] = Array.from({ length: 12 }, (_, i) => ({
      id: String(i),
      type: "T",
      cat: `Cat-${i}`,
      val: 10,
    }));

    const optWithOther = pieOption(manyRows, { x: "cat", y: "val", top: 3 });
    const seriesData = (optWithOther.series as Array<{ data: Array<{ name: string; value: number }> }>)[0].data;
    expect(seriesData).toHaveLength(4);
    expect(seriesData[3]).toEqual({ name: "Other", value: 90 });

    const optNoOther = pieOption(sampleRows, { x: "station", top: 5 });
    const noOtherData = (optNoOther.series as Array<{ data: Array<{ name: string; value: number }> }>)[0].data;
    expect(noOtherData.find((d) => d.name === "Other")).toBeUndefined();
  });

  it("bucketOf truncates hour, day, week (Monday), month and returns null on invalid", () => {
    const iso = "2025-05-14T15:30:45.123Z"; // Wednesday
    expect(bucketOf(iso, "hour")).toBe("2025-05-14T15:00:00.000Z");
    expect(bucketOf(iso, "day")).toBe("2025-05-14T00:00:00.000Z");
    expect(bucketOf(iso, "week")).toBe("2025-05-12T00:00:00.000Z");
    expect(bucketOf(iso, "month")).toBe("2025-05-01T00:00:00.000Z");
    expect(bucketOf("invalid-date", "day")).toBeNull();
  });

  it("timeSeriesOption groups rows by time bucket", () => {
    const opt = timeSeriesOption({
      rows: sampleRows,
      time: "ts",
      y: "bikes",
      bucket: "day",
    });
    const series = (opt.series as Array<{ data: Array<[string, number]> }>)[0];
    expect(series.data).toEqual([["2025-05-14T00:00:00.000Z", 11.666666666666666]]);
    expect(opt.xAxis).toEqual({ type: "time" });
  });

  it("timeSeriesOption handles TemporalRow series dropping non-numeric values", () => {
    const seriesData: TemporalRow[] = [
      {
        id: "sensor-1",
        type: "Sensor",
        series: {
          temp: [
            { observedAt: "2025-05-14T10:00:00Z", value: 21.5 },
            { observedAt: "2025-05-14T11:00:00Z", value: "faulty" },
            { observedAt: "2025-05-14T09:00:00Z", value: 20.0 },
          ],
        },
      },
    ];

    const opt = timeSeriesOption({ series: seriesData, attr: "temp" });
    const line = (opt.series as Array<{ name: string; data: Array<[string, number]> }>)[0];
    expect(line.name).toBe("sensor-1");
    expect(line.data).toEqual([
      ["2025-05-14T09:00:00Z", 20.0],
      ["2025-05-14T10:00:00Z", 21.5],
    ]);
  });
});

describe("ChartCard component", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clickCallback = undefined;
  });

  it("initializes echarts, sets option, handles onSelect, and disposes on unmount", () => {
    const onSelect = vi.fn();
    const testOption = { series: [{ type: "bar", data: [1, 2] }] };

    const { unmount } = render(
      <ChartCard title="Test Chart" option={testOption} onSelect={onSelect} />,
    );

    expect(screen.getByText("Test Chart")).toBeInTheDocument();
    expect(mockChartInstance.setOption).toHaveBeenCalledWith(testOption, true);

    clickCallback?.({ name: "SelectedCategory" });
    expect(onSelect).toHaveBeenCalledWith("SelectedCategory");

    unmount();
    expect(mockChartInstance.dispose).toHaveBeenCalled();
  });

  it("renders Empty when option is null or rows are empty", () => {
    render(<BarChartCard rows={[]} x="station" />);
    expect(
      screen.getByText("Nothing to chart for the current filters."),
    ).toBeInTheDocument();
  });
});
