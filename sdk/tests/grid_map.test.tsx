/**
 * The grid and the map together (UI-72, UI-66, UI-67).
 *
 * The map is stubbed: MapLibre needs a WebGL canvas, which jsdom has none of, and the browser leg
 * of this feature is `e2e/geo_editor.spec.ts` and the grid journey. What is checked here is the
 * panel's own contract — which rows reach the map, which way selection travels, what a pending
 * geometry is reviewed as, and what is written through the Endpoint.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const geoViewCalls: Record<string, unknown>[] = [];
const geoEditorCalls: Record<string, unknown>[] = [];

vi.mock("../src/geo/GeoView", () => ({
  GeoView: (props: Record<string, unknown>) => {
    geoViewCalls.push(props);
    const features = (props.value ?? []) as { id: string }[];
    return (
      <div data-testid="geo-view" data-selected={String(props.selectedId ?? "")}>
        {features.map((feature) => (
          <button key={feature.id} type="button" onClick={() => (props.onSelect as (id: string) => void)?.(feature.id)}>
            shape {feature.id}
          </button>
        ))}
      </div>
    );
  },
}));

vi.mock("../src/geo/GeoEditor", async (importOriginal) => {
  const actual = (await importOriginal()) as Record<string, unknown>;
  return {
    ...actual,
    GeoEditor: (props: Record<string, unknown>) => {
      geoEditorCalls.push(props);
      return (
        <div data-testid="geo-editor">
          <button
            type="button"
            onClick={() =>
              (props.onChange as (g: unknown) => void)({ type: "Point", coordinates: [24.9401, 60.1711] })
            }
          >
            move it
          </button>
        </div>
      );
    },
  };
});

const { GridMap, summaryOf, DEFAULT_GRID_MAP_LABELS } = await import("../src/grid/GridMap");
const { attrsBody } = await import("../src/grid/apply");
const { mapAttrOf } = await import("../src/grid/mapRows");
type RichRow = import("../src/grid/model").RichRow;
type Geometry = import("../src/geo/validate").Geometry;

function station(id: string, coordinates?: [number, number]): RichRow {
  return {
    id,
    type: "BikeHireDockingStation",
    cells: {
      name: { kind: "property", value: id },
      ...(coordinates ? { location: { kind: "geo", value: { type: "Point", coordinates }, geometryType: "Point" } } : {}),
    } as RichRow["cells"],
    raw: {},
  };
}

afterEach(() => {
  cleanup();
  geoViewCalls.length = 0;
  geoEditorCalls.length = 0;
});

describe("the map beside the rows", () => {
  it("draws one shape per row that has a geometry and lists the rows that have none", () => {
    render(
      <GridMap rows={[station("a", [24.9, 60.1]), station("b"), station("c", [25, 60.2])]} attr="location" activeId={null} onActivate={() => {}} />,
    );
    expect(screen.getByRole("button", { name: "shape a" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "shape c" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "shape b" })).toBeNull();
    expect(screen.getByText(`${DEFAULT_GRID_MAP_LABELS.offTheMap}: 1`)).toBeTruthy();
  });

  it("selects both ways: a click on a shape activates its row, an active row draws its shape chosen", async () => {
    const onActivate = vi.fn();
    const { rerender } = render(
      <GridMap rows={[station("a", [24.9, 60.1]), station("b", [25, 60.2])]} attr="location" activeId={null} onActivate={onActivate} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "shape b" }));
    expect(onActivate).toHaveBeenCalledWith("b");

    rerender(
      <GridMap rows={[station("a", [24.9, 60.1]), station("b", [25, 60.2])]} attr="location" activeId="b" onActivate={onActivate} />,
    );
    expect(screen.getByTestId("geo-view").getAttribute("data-selected")).toBe("b");
  });

  it("has no map at all for a type with no geometry, and nothing to act on", () => {
    // The host asks `mapAttrOf` first; a null answer is a page with no panel and no action, rather
    // than an empty canvas offering to draw.
    expect(mapAttrOf([station("a"), station("b")])).toBeNull();
  });

  it("an endpoint that does not answer the geo attribute shows no shapes", () => {
    // The attribute the config names stays the panel's, but a page the Endpoint filtered it out of
    // has nothing to draw: every row is off the map, and none is invented.
    render(<GridMap rows={[station("a"), station("b")]} attr="location" activeId={null} onActivate={() => {}} />);
    expect(screen.queryByRole("button", { name: /^shape/ })).toBeNull();
    expect(screen.getByText(`${DEFAULT_GRID_MAP_LABELS.offTheMap}: 2`)).toBeTruthy();
  });
});

describe("editing a geometry from the grid", () => {
  const rows = [station("a", [24.9, 60.1]), station("b", [25, 60.2])];

  it("is offered only in edit mode, and only to someone who may write", async () => {
    render(<GridMap rows={rows} attr="location" activeId="a" onActivate={() => {}} mode="view" onEdit={() => {}} />);
    expect(screen.queryByRole("button", { name: DEFAULT_GRID_MAP_LABELS.editGeometry })).toBeNull();
    cleanup();

    // A viewer: the grid is in edit mode but hands no writer down, so the map is still readable and
    // the editor is not there to be opened.
    render(<GridMap rows={rows} attr="location" activeId="a" onActivate={() => {}} mode="edit" />);
    expect(screen.getByTestId("geo-view")).toBeTruthy();
    expect(screen.queryByRole("button", { name: DEFAULT_GRID_MAP_LABELS.editGeometry })).toBeNull();
    expect(screen.getByText(DEFAULT_GRID_MAP_LABELS.viewerCannotEdit)).toBeTruthy();
  });

  it("puts the changed geometry into the grid's pending edits, for the row the grid has active", async () => {
    const onEdit = vi.fn();
    render(<GridMap rows={rows} attr="location" activeId="a" onActivate={() => {}} mode="edit" onEdit={onEdit} />);
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_GRID_MAP_LABELS.editGeometry }));
    fireEvent.click(screen.getByRole("button", { name: "move it" }));
    expect(onEdit).toHaveBeenCalledWith("a", "location", { type: "Point", coordinates: [24.9401, 60.1711] });
  });

  it("draws the other rows behind the edited one, to draw against", async () => {
    render(<GridMap rows={rows} attr="location" activeId="a" onActivate={() => {}} mode="edit" onEdit={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_GRID_MAP_LABELS.editGeometry }));
    const snapTo = geoEditorCalls.at(-1)!.snapTo as { id: string }[];
    expect(snapTo.map((f) => f.id)).toEqual(["b"]);
  });

  it("reviews the pending geometry as what it changed", async () => {
    const edits = { a: { location: { type: "Point", coordinates: [24.9, 60.10035] } } };
    render(
      <GridMap rows={rows} attr="location" activeId="a" onActivate={() => {}} mode="edit" onEdit={() => {}} edits={edits} />,
    );
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_GRID_MAP_LABELS.editGeometry }));
    // ~39 m north; the sentence names the shape, the word and the distance.
    expect(screen.getByRole("status").textContent).toMatch(/^Point moved 39 m$/);
  });

  it("shows the pending geometry on the map, not the endpoint's", () => {
    const edits = { a: { location: { type: "Point", coordinates: [11, 12] } } };
    render(<GridMap rows={rows} attr="location" activeId="a" onActivate={() => {}} edits={edits} />);
    const drawn = geoViewCalls.at(-1)!.value as { id: string; geometry: Geometry }[];
    expect(drawn.find((f) => f.id === "a")!.geometry).toEqual({ type: "Point", coordinates: [11, 12] });
  });
});

describe("what the summary says", () => {
  const labels = DEFAULT_GRID_MAP_LABELS;
  const point = (lon: number, lat: number) => ({ type: "Point", coordinates: [lon, lat] }) as Geometry;

  it("measures a small move to a tenth of a metre and a larger one to the metre", () => {
    expect(summaryOf(point(24.9, 60.1), point(24.9, 60.10002), labels)).toBe("Point moved 2.2 m");
    expect(summaryOf(point(24.9, 60.1), point(24.9, 60.101), labels)).toBe("Point moved 111 m");
  });

  it("counts vertices and area for a reshaped polygon, in the singular where there is one", () => {
    const before = { type: "Polygon", coordinates: [[[0, 0], [0.001, 0], [0.001, 0.001], [0, 0.001], [0, 0]]] } as Geometry;
    const after = { type: "Polygon", coordinates: [[[0, 0], [0.001, 0], [0.0015, 0.0005], [0.001, 0.001], [0, 0.001], [0, 0]]] } as Geometry;
    const said = summaryOf(before, after, labels);
    expect(said).toContain("Polygon reshaped: 1 vertex added");
    expect(said).toMatch(/area \+\d+ %/);
  });

  it("says replaced rather than measuring between two kinds of shape", () => {
    const line = { type: "LineString", coordinates: [[0, 0], [1, 1]] } as Geometry;
    expect(summaryOf(point(0, 0), line, labels)).toBe("replaced: Point → LineString");
  });

  it("says nothing when nothing changed", () => {
    expect(summaryOf(point(0, 0), point(0, 0), labels)).toBe("");
  });
});

describe("what the Endpoint receives", () => {
  it("writes an edited geometry as a GeoProperty, so the attribute keeps its type", () => {
    const geometry = { type: "Point", coordinates: [24.9401, 60.1711] };
    const body = attrsBody([{ attribute: "location", after: geometry, kind: "geo" }], "keep");
    expect(body.location).toEqual({ type: "GeoProperty", value: geometry });
  });

  it("leaves observedAt alone unless the batch says the value was observed now", () => {
    const geometry = { type: "Point", coordinates: [24.9401, 60.1711] };
    const kept = attrsBody([{ attribute: "location", after: geometry, kind: "geo" }], "keep");
    expect(kept.location).not.toHaveProperty("observedAt");
    const now = attrsBody([{ attribute: "location", after: geometry, kind: "geo" }], "now", () => "2026-09-19T00:00:00Z");
    expect((now.location as Record<string, unknown>).observedAt).toBe("2026-09-19T00:00:00Z");
  });

  it("carries no unit on a geometry, because a shape is not measured in one", () => {
    const body = attrsBody(
      [{ attribute: "location", after: { type: "Point", coordinates: [0, 0] }, kind: "geo", unitCode: "MTR" }],
      "keep",
    );
    expect(body.location).not.toHaveProperty("unitCode");
  });
});

describe("the drawn area as a filter", () => {
  const rows = [station("a", [24.9, 60.1])];

  it("asks for the area a person drew, on the attribute the map is showing", async () => {
    const onArea = vi.fn();
    render(
      <GridMap
        rows={rows}
        attr="location"
        activeId={null}
        onActivate={() => {}}
        onArea={onArea}
        boundsNow={() => [24.8, 60.0, 25.1, 60.3]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_GRID_MAP_LABELS.drawArea }));
    expect(onArea).toHaveBeenCalledWith({
      geoproperty: "location",
      geometry: {
        type: "Polygon",
        coordinates: [[[24.8, 60], [25.1, 60], [25.1, 60.3], [24.8, 60.3], [24.8, 60]]],
      },
    });
  });

  it("stays visible while it filters, and is removed by the same control", async () => {
    const onArea = vi.fn();
    const area = {
      geoproperty: "location",
      geometry: { type: "Polygon", coordinates: [[[24.8, 60], [25.1, 60], [25.1, 60.3], [24.8, 60.3], [24.8, 60]]] } as Geometry,
    };
    render(
      <GridMap rows={rows} attr="location" activeId={null} onActivate={() => {}} area={area} onArea={onArea} boundsNow={() => null} />,
    );
    expect(screen.getByRole("status").textContent).toContain(DEFAULT_GRID_MAP_LABELS.areaAsked);
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_GRID_MAP_LABELS.clearArea }));
    expect(onArea).toHaveBeenCalledWith(null);
  });

  it("says so when the filter is no longer the shape that was drawn", () => {
    const ring: [number, number][] = [];
    for (let i = 0; i < 3000; i += 1) {
      const angle = (i / 3000) * Math.PI * 2;
      ring.push([24.9412345 + Math.cos(angle) / 100, 60.1712345 + Math.sin(angle) / 100]);
    }
    ring.push(ring[0]);
    render(
      <GridMap
        rows={rows}
        attr="location"
        activeId={null}
        onActivate={() => {}}
        area={{ geoproperty: "location", geometry: { type: "Polygon", coordinates: [ring] } as Geometry }}
        onArea={() => {}}
        boundsNow={() => null}
      />,
    );
    expect(screen.getByRole("status").textContent).toContain(DEFAULT_GRID_MAP_LABELS.areaSimplified);
  });

  it("offers no area control where the host cannot take one", () => {
    render(<GridMap rows={rows} attr="location" activeId={null} onActivate={() => {}} />);
    expect(screen.queryByRole("button", { name: DEFAULT_GRID_MAP_LABELS.drawArea })).toBeNull();
  });
});
