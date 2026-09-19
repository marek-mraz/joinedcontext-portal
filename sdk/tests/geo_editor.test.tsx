import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * T-1442, UI-72, SDK-29: the editor without a canvas. The map is mocked the way `map.test.tsx`
 * mocks it, so what these tests read is the part that has to work by keyboard — the toolbar, the
 * coordinate table, the paste and the check between them and `onChange`.
 */
const maps: Array<{ fitBounds: ReturnType<typeof vi.fn> }> = [];
let loadHandler: (() => void) | undefined;

vi.mock("maplibre-gl", () => {
  class Map {
    fitBounds = vi.fn();
    setData = vi.fn();
    constructor() {
      maps.push({ fitBounds: this.fitBounds });
    }
    on(event: string, ...args: unknown[]) {
      if (event === "load" && typeof args[0] === "function") {
        loadHandler = args[0] as () => void;
      }
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData: this.setData });
    remove = vi.fn();
  }
  return { Map, setWorkerUrl: vi.fn() };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { GeoEditor, modesFor, rowsOf, withPosition, withoutPosition } = await import("../src/geo/GeoEditor");
type Geometry = import("../src/geo/validate").Geometry;

const SQUARE: Geometry = {
  type: "Polygon",
  coordinates: [
    [
      [24.9, 60.1],
      [24.95, 60.1],
      [24.95, 60.15],
      [24.9, 60.15],
      [24.9, 60.1],
    ],
  ],
};

beforeEach(() => {
  maps.length = 0;
  loadHandler = undefined;
});

describe("the toolbar", () => {
  it("offers only the modes the host allows", () => {
    expect(modesFor(["Point"]).map((m) => m.name)).toEqual(["point"]);
    expect(modesFor(["Polygon"]).map((m) => m.name)).toEqual(["polygon", "rectangle", "circle", "freehand"]);
  });

  it("shows those modes and no others, plus select, undo, redo and delete", () => {
    render(<GeoEditor value={null} onChange={vi.fn()} allowed={["Point"]} />);
    const toolbar = screen.getByRole("toolbar", { name: "Drawing tools" });
    expect(within(toolbar).getByRole("button", { name: "Point" })).toBeTruthy();
    expect(within(toolbar).queryByRole("button", { name: "Area" })).toBeNull();
    for (const name of ["Select", "Undo", "Redo", "Delete the shape"]) {
      expect(within(toolbar).getByRole("button", { name })).toBeTruthy();
    }
  });
});

describe("the coordinate table, which is the keyboard's way in", () => {
  it("reads every position of a polygon, ring and index named", () => {
    const rows = rowsOf(SQUARE);
    expect(rows).toHaveLength(5);
    expect(rows[0].at).toEqual([0, 0]);
    expect(rows[4].position).toEqual([24.9, 60.1]);
  });

  it("emits the corrected geometry when a longitude is typed, checked first", async () => {
    const onChange = vi.fn();
    render(<GeoEditor value={SQUARE} onChange={onChange} />);
    const input = screen.getByLabelText("Point 1.1 longitude");
    fireEvent.change(input, { target: { value: "24.8" } });
    fireEvent.blur(input);
    expect(onChange).toHaveBeenCalledTimes(1);
    const emitted = onChange.mock.calls[0][0] as Geometry;
    // The moved corner, and the ring closed back on it: moving the first point unclosed the ring,
    // so the check appended the closing position rather than refusing the edit.
    const ring = (emitted.coordinates as number[][][])[0];
    expect(ring[0]).toEqual([24.8, 60.1]);
    expect(ring[ring.length - 1]).toEqual(ring[0]);
    expect(ring).toHaveLength(6);
  });

  it("says why nothing was written when the typed pair is out of bounds, and writes nothing", async () => {
    const onChange = vi.fn();
    render(<GeoEditor value={{ type: "Point", coordinates: [24.9, 60.1] }} onChange={onChange} />);
    const input = screen.getByLabelText("Point latitude");
    fireEvent.change(input, { target: { value: "600" } });
    fireEvent.blur(input);
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/latitude 600 is outside ±90/);
  });

  it("removes a vertex from the table and keeps the rest", async () => {
    const onChange = vi.fn();
    render(<GeoEditor value={SQUARE} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove point 1.3" }));
    const emitted = onChange.mock.calls[0][0] as Geometry;
    expect((emitted.coordinates as number[][][])[0]).toHaveLength(4);
  });

  it("refuses a removal that would leave a ring with two corners, and says so", async () => {
    const triangle: Geometry = {
      type: "Polygon",
      coordinates: [
        [
          [0, 0],
          [1, 0],
          [1, 1],
          [0, 0],
        ],
      ],
    };
    const onChange = vi.fn();
    render(<GeoEditor value={triangle} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove point 1.2" }));
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toBeTruthy();
  });

  it("writes a position back without touching any other number, altitude included", () => {
    const withZ: Geometry = {
      type: "LineString",
      coordinates: [
        [1, 2, 300],
        [3, 4, 500],
      ],
    };
    expect(withPosition(withZ, [1], [9, 8, 500]).coordinates).toEqual([
      [1, 2, 300],
      [9, 8, 500],
    ]);
    expect(withoutPosition(withZ, [0]).coordinates).toEqual([[3, 4, 500]]);
  });
});

describe("a geometry from somewhere else", () => {
  it("takes a pasted Feature and clears the box", () => {
    const onChange = vi.fn();
    render(<GeoEditor value={null} onChange={onChange} allowed={["Point"]} />);
    const box = screen.getByLabelText("Paste a geometry, a Feature or a FeatureCollection");
    fireEvent.change(box, { target: { value: JSON.stringify({ type: "Feature", geometry: { type: "Point", coordinates: [24.9, 60.1] } }) } });
    fireEvent.click(screen.getByRole("button", { name: "Take it" }));
    expect(onChange).toHaveBeenCalledWith({ type: "Point", coordinates: [24.9, 60.1] });
    expect((box as HTMLTextAreaElement).value).toBe("");
  });

  it("says a paste is not JSON instead of throwing", async () => {
    const onChange = vi.fn();
    render(<GeoEditor value={null} onChange={onChange} />);
    const box = screen.getByLabelText("Paste a geometry, a Feature or a FeatureCollection");
    fireEvent.change(box, { target: { value: "POINT(24.9 60.1)" } });
    fireEvent.click(screen.getByRole("button", { name: "Take it" }));
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/that is not JSON/);
  });

  it("refuses a pasted type the host does not allow", async () => {
    const onChange = vi.fn();
    render(<GeoEditor value={null} onChange={onChange} allowed={["Polygon"]} />);
    const box = screen.getByLabelText("Paste a geometry, a Feature or a FeatureCollection");
    fireEvent.change(box, { target: { value: JSON.stringify({ type: "Point", coordinates: [1, 2] }) } });
    fireEvent.click(screen.getByRole("button", { name: "Take it" }));
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/Point is not one of Polygon here/);
  });
});

describe("the drawing library", () => {
  it("is started with the allowed modes and select, and a finished shape is checked", async () => {
    const drawn: Geometry = { type: "Point", coordinates: [24.9, 60.1] };
    const handlers: Record<string, () => void> = {};
    const drawing = {
      start: vi.fn(),
      stop: vi.fn(),
      setMode: vi.fn(),
      addFeatures: vi.fn(),
      getSnapshot: () => [{ geometry: drawn }],
      clear: vi.fn(),
      undo: vi.fn(),
      redo: vi.fn(),
      on: (event: string, callback: () => void) => {
        handlers[event] = callback;
      },
    };
    const engine = { create: vi.fn((_map: unknown, _modes: string[]) => drawing) };
    const onChange = vi.fn();
    render(<GeoEditor value={null} onChange={onChange} allowed={["Point"]} engine={engine} />);
    await waitFor(() => expect(loadHandler).toBeTruthy());
    loadHandler?.();
    // The library is reached through a promise, because the real one is imported on map load.
    await waitFor(() => expect(drawing.start).toHaveBeenCalled());
    expect(engine.create.mock.calls[0][1]).toEqual(["point", "select"]);

    handlers.finish?.();
    expect(onChange).toHaveBeenCalledWith(drawn);

    fireEvent.click(screen.getByRole("button", { name: "Select" }));
    expect(drawing.setMode).toHaveBeenCalledWith("select");
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(drawing.undo).toHaveBeenCalled();
  });

  it("clears the drawing and the value together when the shape is deleted", async () => {
    const drawing = {
      start: vi.fn(),
      stop: vi.fn(),
      setMode: vi.fn(),
      addFeatures: vi.fn(),
      getSnapshot: () => [],
      clear: vi.fn(),
      undo: vi.fn(),
      redo: vi.fn(),
      on: vi.fn(),
    };
    const onChange = vi.fn();
    render(
      <GeoEditor value={SQUARE} onChange={onChange} engine={{ create: () => drawing }} />,
    );
    await waitFor(() => expect(loadHandler).toBeTruthy());
    loadHandler?.();
    await waitFor(() => expect(drawing.start).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Delete the shape" }));
    expect(drawing.clear).toHaveBeenCalled();
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("edits the table with no engine at all, which is what a test and a server render get", async () => {
    const onChange = vi.fn();
    render(<GeoEditor value={{ type: "Point", coordinates: [1, 2] }} onChange={onChange} />);
    const input = screen.getByLabelText("Point longitude");
    fireEvent.change(input, { target: { value: "3" } });
    fireEvent.blur(input);
    expect(onChange).toHaveBeenCalledWith({ type: "Point", coordinates: [3, 2] });
  });
});
