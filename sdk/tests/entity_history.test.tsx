/**
 * How one value got to where it is (T-1431; UI-66, EP-07).
 *
 * The panel reads through the grid's own source, so the cases are about what it asks for and what
 * it tells a person: the window in UTC, the cap on a single read, a series that was cut said to be
 * cut, a text attribute with no line drawn over it, and a refusal in the endpoint's own words.
 */
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, SourceError } from "../src/grid/source";
import type { HistoryPoint, HistoryWindow } from "../src/grid/source";
import {
  asCsv,
  DEFAULT_HISTORY_LABELS as L,
  EntityHistory,
  MAX_POINTS,
  polyline,
  windowOf,
} from "../src/grid/EntityHistory";

const ID = "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001";

const NUMBERS: HistoryPoint[] = [
  { at: "2026-09-19T08:00:00Z", value: 5 },
  { at: "2026-09-19T09:00:00Z", value: 3 },
  { at: "2026-09-19T10:00:00Z", value: 8 },
];

/** A source whose history answers what a test gives it, and records the window it was asked for. */
function withHistory(
  answer: (window: HistoryWindow) => Promise<HistoryPoint[]> = async () => NUMBERS,
) {
  const asked: HistoryWindow[] = [];
  return {
    asked,
    source: {
      history: async (_id: string, _attr: string, window: HistoryWindow) => {
        asked.push(window);
        return answer(window);
      },
    },
  };
}

describe("one attribute's history", () => {
  it("asks for the chosen window in UTC, capped, and draws the numbers it got", async () => {
    const { asked, source } = withHistory();
    render(
      <EntityHistory
        source={source}
        id={ID}
        attr="availableBikeNumber"
        unit="C62"
        now={() => new Date("2026-09-19T10:00:00Z")}
      />,
    );

    await waitFor(() => expect(asked).toHaveLength(1));
    // A day back from now, both ends in UTC, and never more points than one read may ask for.
    expect(asked[0]).toEqual({
      from: "2026-09-18T10:00:00.000Z",
      to: "2026-09-19T10:00:00.000Z",
      lastN: MAX_POINTS,
    });

    const panel = screen.getByRole("region", { name: /History availableBikeNumber/ });
    // The entity it is about, the values, and the unit they are measured in.
    expect(panel.textContent).toContain(ID);
    expect(within(panel).getByText("2026-09-19T09:00:00Z")).toBeInTheDocument();
    expect(within(panel).getAllByText("C62").length).toBeGreaterThan(0);
    expect(within(panel).getByRole("img", { name: /History availableBikeNumber/ })).toBeInTheDocument();
  });

  it("reads the window a person picks, and asks again for it", async () => {
    const { asked, source } = withHistory();
    render(
      <EntityHistory source={source} id={ID} attr="availableBikeNumber" now={() => new Date("2026-09-19T10:00:00Z")} />,
    );
    await waitFor(() => expect(asked).toHaveLength(1));

    fireEvent.click(screen.getByLabelText(L.window.hour));
    await waitFor(() => expect(asked).toHaveLength(2));
    expect(asked[1].from).toBe("2026-09-19T09:00:00.000Z");

    fireEvent.click(screen.getByLabelText(L.window.custom));
    fireEvent.change(screen.getByLabelText(L.from), { target: { value: "2026-09-01T00:00" } });
    await waitFor(() => expect(asked.at(-1)?.from).toBe(new Date("2026-09-01T00:00").toISOString()));
  });

  it("says so when nothing was recorded, and when the series was cut", async () => {
    const { source } = withHistory(async () => []);
    const { unmount } = render(<EntityHistory source={source} id={ID} attr="availableBikeNumber" />);
    expect(await screen.findByText(L.empty)).toBeInTheDocument();
    // Nothing to draw and nothing to copy.
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.queryByRole("button", { name: L.copy })).toBeNull();
    unmount();

    const full = withHistory(async () => NUMBERS.slice(0, 2));
    render(<EntityHistory source={full.source} id={ID} attr="availableBikeNumber" maxPoints={2} />);
    expect(await screen.findByText(L.cut)).toBeInTheDocument();
    expect(full.asked[0].lastN).toBe(2);
  });

  it("shows a text attribute as a table and draws no line over it", async () => {
    const { source } = withHistory(async () => [
      { at: "2026-09-19T08:00:00Z", value: "Kamppi" },
      { at: "2026-09-19T09:00:00Z", value: "Kamppi (west)" },
    ]);
    render(<EntityHistory source={source} id={ID} attr="name" />);
    expect(await screen.findByText("Kamppi (west)")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("shows the endpoint's own sentence when the read is refused", async () => {
    const { source } = withHistory(async () => {
      throw new SourceError(403, "temperature is not in what you may read");
    });
    render(<EntityHistory source={source} id={ID} attr="temperature" />);
    expect(await screen.findByText(/temperature is not in what you may read/)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("copies the series as CSV a spreadsheet can read", async () => {
    const written: string[] = [];
    vi.stubGlobal("navigator", { clipboard: { writeText: (text: string) => written.push(text) } });
    const { source } = withHistory();
    render(<EntityHistory source={source} id={ID} attr="availableBikeNumber" />);
    fireEvent.click(await screen.findByRole("button", { name: L.copy }));
    expect(written[0]).toBe(
      "observedAt,availableBikeNumber\n2026-09-19T08:00:00Z,5\n2026-09-19T09:00:00Z,3\n2026-09-19T10:00:00Z,8",
    );
    vi.unstubAllGlobals();

    // A value with a comma or a quote in it stays one cell.
    expect(asCsv([{ at: "t", value: 'a,b"c' }], "n")).toBe('observedAt,n\nt,"a,b""c"');
  });

  it("draws nothing from fewer than two numbers, and puts the largest value at the top", () => {
    expect(polyline([])).toBeNull();
    expect(polyline([{ at: "2026-09-19T08:00:00Z", value: 5 }])).toBeNull();
    expect(polyline([{ at: "t", value: "x" }, { at: "u", value: "y" }])).toBeNull();
    const line = polyline(NUMBERS)!;
    const ys = line.split(" ").map((pair) => Number(pair.split(",")[1]));
    // 5, 3, 8: the largest (8) is at y=0, the smallest (3) at y=100.
    expect(ys[2]).toBe(0);
    expect(ys[1]).toBe(100);
  });

  it("never asks for more than the cap, whatever the config says", () => {
    expect(windowOf("day", { from: "", to: "" }, 5000).lastN).toBe(5000);
    const { source, asked } = withHistory();
    render(<EntityHistory source={source} id={ID} attr="availableBikeNumber" maxPoints={5000} />);
    return waitFor(() => expect(asked[0].lastN).toBe(MAX_POINTS));
  });
});

describe("the grid's own way in", () => {
  // History is asked for by the config, because a temporal read costs the broker a query the
  // grid's own page does not (`history: { enabled: false }` is what an unconfigured grid resolves
  // to, so both halves have to say yes).
  const config = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: "BikeHireDockingStation",
    columns: [{ attr: "availableBikeNumber", label: "Bikes" }],
    history: { enabled: true },
    pageSize: 10,
  }).config!;

  const entities = [
    {
      id: ID,
      type: "BikeHireDockingStation",
      availableBikeNumber: { type: "Property", value: 5, unitCode: "C62" },
    },
  ];

  it("offers history where the attribute's metadata already is, and only when the source answers it", async () => {
    const plain = fixtureSource(entities);
    const { unmount } = render(<EntityGrid config={config} source={plain} />);
    await waitFor(() => expect(screen.getByText("5 C62")).toBeInTheDocument());
    fireEvent.click(screen.getByLabelText("Show metadata for availableBikeNumber"));
    // A source that cannot answer a temporal read offers nothing to click (SDK-29).
    expect(screen.queryByRole("button", { name: "History" })).toBeNull();
    unmount();

    const temporal = { ...plain, history: async () => NUMBERS };
    render(<EntityGrid config={config} source={temporal} />);
    await waitFor(() => expect(screen.getByText("5 C62")).toBeInTheDocument());
    fireEvent.click(screen.getByLabelText("Show metadata for availableBikeNumber"));
    fireEvent.click(screen.getByRole("button", { name: "History" }));

    const panel = await screen.findByRole("region", { name: /History availableBikeNumber/ });
    expect(panel.textContent).toContain(ID);
    fireEvent.click(within(panel).getByRole("button", { name: L.close }));
    expect(screen.queryByRole("region", { name: /History availableBikeNumber/ })).toBeNull();
  });

  it("offers nothing when the config never asked for history", async () => {
    const off = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "availableBikeNumber", label: "Bikes" }],
      pageSize: 10,
    }).config!;
    render(<EntityGrid config={off} source={{ ...fixtureSource(entities), history: async () => NUMBERS }} />);
    await waitFor(() => expect(screen.getByText("5 C62")).toBeInTheDocument());
    fireEvent.click(screen.getByLabelText("Show metadata for availableBikeNumber"));
    expect(screen.queryByRole("button", { name: "History" })).toBeNull();
  });
});
