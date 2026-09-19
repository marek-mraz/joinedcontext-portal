/**
 * The drawn area as an NGSI-LD geo query (UI-72, UI-66; CIM 009 §4.10).
 */
import { describe, expect, it } from "vitest";

import {
  areaQuery,
  decimate,
  MAX_COORDINATES_CHARS,
  ringOf,
  ringOfBounds,
} from "../src/grid/geoarea";
import type { Geometry, Position } from "../src/geo/validate";

const square: Geometry = {
  type: "Polygon",
  coordinates: [ringOfBounds(24.9, 60.1, 25.0, 60.2)],
} as Geometry;

/** A ring of `n` vertices around Helsinki, closed, with positions no rounding can shorten. */
function manyVertices(n: number): Geometry {
  const ring: Position[] = [];
  for (let i = 0; i < n; i += 1) {
    const angle = (i / n) * Math.PI * 2;
    ring.push([24.9412345 + Math.cos(angle) / 100, 60.1712345 + Math.sin(angle) / 100]);
  }
  ring.push(ring[0]);
  return { type: "Polygon", coordinates: [ring] } as Geometry;
}

describe("the drawn area becomes a geo query", () => {
  it("asks within the polygon, for the attribute the map is showing", () => {
    const query = areaQuery({ geoproperty: "location", geometry: square });
    expect(query).not.toBeNull();
    expect(query!.georel).toBe("within");
    expect(query!.geometry).toBe("Polygon");
    expect(query!.geoproperty).toBe("location");
    expect(query!.simplified).toBe(false);
    expect(JSON.parse(query!.coordinates)).toEqual([ringOfBounds(24.9, 60.1, 25.0, 60.2)]);
  });

  it("survives the URL: it stays under the limit and says when it stopped being the shape", () => {
    const drawn = areaQuery({ geoproperty: "location", geometry: manyVertices(4000) });
    expect(drawn).not.toBeNull();
    expect(drawn!.coordinates.length).toBeLessThanOrEqual(MAX_COORDINATES_CHARS);
    expect(drawn!.simplified).toBe(true);
    // Still a closed ring the endpoint will accept, not a line the loop cut open.
    const ring = JSON.parse(drawn!.coordinates)[0] as Position[];
    expect(ring.length).toBeGreaterThanOrEqual(4);
    expect(ring[0]).toEqual(ring[ring.length - 1]);
  });

  it("rounds to the precision a drawn shape actually has", () => {
    const exact: Geometry = {
      type: "Polygon",
      coordinates: [
        [
          [24.9123456789, 60.1123456789],
          [25.0, 60.1],
          [25.0, 60.2],
          [24.9123456789, 60.1123456789],
        ],
      ],
    } as Geometry;
    const ring = ringOf(exact)!;
    expect(ring[0]).toEqual([24.912346, 60.112346]);
  });

  it("keeps a short ring exactly as drawn", () => {
    const ring = ringOfBounds(0, 0, 1, 1);
    expect(decimate(ring, MAX_COORDINATES_CHARS)).toEqual({ ring, simplified: false });
  });

  it("refuses what is not an area, instead of asking for everything", () => {
    expect(areaQuery(null)).toBeNull();
    expect(areaQuery(undefined)).toBeNull();
    // A point or a line is not an area: `within` a Point matches nothing and would read as an
    // endpoint that answered empty.
    const point = { type: "Point", coordinates: [24.9, 60.1] } as Geometry;
    expect(areaQuery({ geoproperty: "location", geometry: point })).toBeNull();
    const line = { type: "LineString", coordinates: [[24.9, 60.1], [25, 60.2]] } as Geometry;
    expect(areaQuery({ geoproperty: "location", geometry: line })).toBeNull();
    // A ring of three positions is not closed and is no area either.
    const open = { type: "Polygon", coordinates: [[[0, 0], [1, 0], [0, 0]]] } as Geometry;
    expect(areaQuery({ geoproperty: "location", geometry: open })).toBeNull();
    // An attribute nobody named cannot be asked about.
    expect(areaQuery({ geoproperty: "  ", geometry: square })).toBeNull();
  });

  it("draws the rectangle counter-clockwise and closed", () => {
    const ring = ringOfBounds(24.9, 60.1, 25.0, 60.2);
    expect(ring).toHaveLength(5);
    expect(ring[0]).toEqual([24.9, 60.1]);
    expect(ring[4]).toEqual(ring[0]);
    // Counter-clockwise: the shoelace sum of an exterior ring is positive (RFC 7946 §3.1.6).
    let area = 0;
    for (let i = 0; i < ring.length - 1; i += 1) {
      area += ring[i][0] * ring[i + 1][1] - ring[i + 1][0] * ring[i][1];
    }
    expect(area).toBeGreaterThan(0);
  });
});
