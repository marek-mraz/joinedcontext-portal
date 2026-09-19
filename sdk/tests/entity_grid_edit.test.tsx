/**
 * Edit mode in the grid (T-1430; UI-67, EP-55, AG-78, UI-60): a person corrects one value by hand,
 * reads what they are about to send, and the Endpoint decides it.
 *
 * Every case is about the two things a person can lose: the value they typed, and the truth about
 * whether it landed. A refused update therefore keeps its cell and its reason, an applied one is
 * re-read from the endpoint, and nothing is sent for a surface that takes no writes.
 */
import { describe, expect, it } from "vitest";
import { render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, SourceError } from "../src/grid/source";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";

const entities: Record<string, unknown>[] = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 5, unitCode: "C62", observedAt: "2026-01-01T10:00:00Z" },
    name: { type: "Property", value: "Kamppi" },
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 3, unitCode: "C62" },
    name: { type: "Property", value: "Kallio" },
  },
];

function configOf(extra: Record<string, unknown> = {}) {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: "BikeHireDockingStation",
    columns: [
      { attr: "name", label: "Name" },
      { attr: "availableBikeNumber", label: "Bikes", show: { unit: true } },
    ],
    mode: "edit",
    // `parseGridConfig` requires the list in edit mode: a grid opens the columns someone named.
    editableAttrs: ["availableBikeNumber", "name"],
    pageSize: 10,
    ...extra,
  });
  expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
  return parsed.config!;
}

/** The fixture source plus the one capability edit mode needs, recording what it is asked to write. */
function writable(refuse?: (id: string) => SourceError | undefined) {
  const inner = fixtureSource(entities);
  const written: { id: string; attrs: Record<string, unknown> }[] = [];
  const reads = { count: 0 };
  return {
    written,
    reads,
    source: {
      query: async (...args: Parameters<typeof inner.query>) => {
        reads.count += 1;
        return inner.query(...args);
      },
      get: inner.get,
      patch: async (id: string, attrs: Record<string, unknown>) => {
        written.push({ id, attrs });
        const problem = refuse?.(id);
        if (problem) {
          throw problem;
        }
      },
    },
  };
}

describe("edit mode", () => {
  it("edits a cell, reviews it, and sends one update the endpoint decides", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());

    // The person corrects one value; nothing is sent while they type.
    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    expect(written).toHaveLength(0);
    expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument();

    // What they are about to send, in the words of the grid they were reading.
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    expect(panel.textContent).toContain("availableBikeNumber");
    expect(panel.textContent).toContain("urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001");

    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));
    await waitFor(() => expect(written).toHaveLength(1));
    // One entity, one PATCH, the number as a number and the unit kept.
    expect(written[0]).toEqual({
      id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
      attrs: { availableBikeNumber: { type: "Property", value: 4, unitCode: "C62" } },
    });
    // Applied cells are no longer pending, and the endpoint's own answer is read again.
    await waitFor(() => expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull());
  });

  it("dates the corrected values to now when the person says they were observed now", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByLabelText(DEFAULT_LABELS.observedNow));
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));

    await waitFor(() => expect(written).toHaveLength(1));
    const sent = written[0].attrs.availableBikeNumber as { observedAt?: string };
    expect(typeof sent.observedAt).toBe("string");
    expect(new Date(sent.observedAt!).getTime()).not.toBeNaN();
  });

  it("keeps a refused cell and its reason, and forgets the one that landed", async () => {
    const second = "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002";
    const { written, source } = writable((id) =>
      id === second ? new SourceError(403, "your role may not write availableBikeNumber") : undefined,
    );
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.change(screen.getByDisplayValue("3"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));

    await waitFor(() => expect(written).toHaveLength(2));
    // The endpoint's own sentence, where the person can see it, and the value still theirs.
    expect(await screen.findByText(new RegExp("may not write availableBikeNumber"))).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument());
    expect(screen.getByDisplayValue("2")).toBeInTheDocument();
  });

  it("forgets a cell typed back to what the endpoint answered", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument();
    fireEvent.change(screen.getByDisplayValue("4"), { target: { value: "5" } });
    // Nothing pending, so Apply is not even offered: there is nothing to apply.
    expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull();
    expect(screen.queryByRole("button", { name: DEFAULT_LABELS.review })).toBeNull();
    expect(written).toHaveLength(0);
  });

  it("discards every pending cell when the person asks", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.discard }));
    expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull();
    expect(screen.getByDisplayValue("5")).toBeInTheDocument();
    expect(written).toHaveLength(0);
  });

  it("offers no editing on a surface that takes no writes, or outside edit mode", async () => {
    // A viewer reads through a source without `patch`: there is nothing to type into.
    render(<EntityGrid config={configOf()} source={fixtureSource(entities)} />);
    await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
    expect(screen.queryByLabelText(`${DEFAULT_LABELS.edit} Bikes`)).toBeNull();

    const { source } = writable();
    render(<EntityGrid config={configOf({ mode: "view", editableAttrs: [] })} source={source} />);
    await waitFor(() => expect(screen.getAllByText("Kallio").length).toBeGreaterThan(0));
    expect(screen.queryByLabelText(`${DEFAULT_LABELS.edit} Bikes`)).toBeNull();
  });

  it("edits only the attributes the config names", async () => {
    const { source } = writable();
    render(<EntityGrid config={configOf({ editableAttrs: ["availableBikeNumber"] })} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());
    // The name is read: a grid whose config opens one column does not open the rest.
    expect(screen.queryByLabelText(`${DEFAULT_LABELS.edit} Name`)).toBeNull();
    expect(screen.getByText("Kamppi")).toBeInTheDocument();
  });

  it("sends a value that looks like markup as data, and renders it as text", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf({ editableAttrs: ["name"] })} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("Kamppi"), { target: { value: "<b>Kamppi</b>" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    fireEvent.click(
      within(screen.getByRole("region", { name: DEFAULT_LABELS.review })).getByRole("button", {
        name: DEFAULT_LABELS.apply,
      }),
    );
    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0].attrs).toEqual({ name: { type: "Property", value: "<b>Kamppi</b>" } });
    // As text: the markup is a value, never markup of the page.
    expect(document.querySelector("tbody b")).toBeNull();
  });

  it("refuses to apply more entities than the platform's own cap", async () => {
    const many = Array.from({ length: 51 }, (_, i) => ({
      id: `urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:${String(i).padStart(3, "0")}`,
      type: "BikeHireDockingStation",
      availableBikeNumber: { type: "Property", value: i },
    }));
    const inner = fixtureSource(many);
    const written: string[] = [];
    const source = {
      query: inner.query,
      get: inner.get,
      patch: async (id: string) => {
        written.push(id);
      },
    };
    render(
      <EntityGrid
        config={{ ...configOf({ editableAttrs: ["availableBikeNumber"] }), pageSize: 51 }}
        source={source}
      />,
    );
    await waitFor(() => expect(screen.getAllByLabelText(`${DEFAULT_LABELS.edit} Bikes`)).toHaveLength(51));

    for (const input of screen.getAllByLabelText(`${DEFAULT_LABELS.edit} Bikes`)) {
      fireEvent.change(input, { target: { value: "999" } });
    }
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    // Refused before anything is sent: half a batch is worse than none.
    expect(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply })).toBeDisabled();
    expect(written).toHaveLength(0);
  });

  it("re-reads the endpoint after applying, so the grid shows its answer and not the guess", async () => {
    const { source, reads } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());
    const before = reads.count;

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    fireEvent.click(
      within(screen.getByRole("region", { name: DEFAULT_LABELS.review })).getByRole("button", {
        name: DEFAULT_LABELS.apply,
      }),
    );
    await waitFor(() => expect(reads.count).toBeGreaterThan(before));
  });
});

describe("a geometry cell in edit mode (T-1443, UI-72)", () => {
  const withGeometry = [
    {
      id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
      type: "BikeHireDockingStation",
      name: { type: "Property", value: "Kamppi" },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.931, 60.169] } },
    },
  ];

  function geoConfig() {
    const parsed = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }, { attr: "location", label: "Location" }],
      mode: "edit",
      editableAttrs: ["name", "location"],
      pageSize: 10,
    });
    expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
    return parsed.config!;
  }

  function source() {
    const inner = fixtureSource(withGeometry);
    return { ...inner, patch: async () => {} };
  }

  it("offers no text box over a geometry, because a text box would show and write [object Object]", async () => {
    render(<EntityGrid config={geoConfig()} source={source()} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());
    // The scalar column opens; the geometry column does not.
    expect(screen.queryByRole("textbox", { name: /Location/ })).toBeNull();
    expect(screen.queryByDisplayValue("[object Object]")).toBeNull();
    // The cell still says what it holds, so the table stays the record of the row.
    expect(screen.getByText("Point")).toBeInTheDocument();
  });
});
