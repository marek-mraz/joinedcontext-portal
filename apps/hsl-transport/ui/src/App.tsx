import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource, StyleSpecification } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import {
  featureCollection,
  getVehicles,
  lineColor,
  subscribe,
  type Vehicle,
  type VehicleCollection,
} from "./api";

const SOURCE = "vehicles";
const HELSINKI: [number, number] = [24.94, 60.17];

/**
 * A keyless raster basemap with its attribution, so the app runs on a cluster with no map
 * account and no key anywhere in the manifest.
 */
const STYLE: StyleSpecification = {
  version: 8,
  sources: {
    osm: {
      type: "raster",
      tiles: ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"],
      tileSize: 256,
      maxzoom: 19,
      attribution: '© <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>',
    },
  },
  layers: [{ id: "osm", type: "raster", source: "osm" }],
};

/**
 * The fleet, seeded from the backend's snapshot and kept current by the stream. A message
 * carries only what moved, so it is merged into what is already on the map rather than
 * replacing it (AP-41).
 */
export function useVehicles(): { vehicles: Vehicle[]; live: boolean } {
  const [byId, setById] = useState<Map<string, Vehicle>>(new Map());
  const [live, setLive] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const merge = (batch: Vehicle[]) => {
      if (cancelled || batch.length === 0) {
        return;
      }
      setById((current) => {
        const next = new Map(current);
        for (const vehicle of batch) {
          next.set(vehicle.id, vehicle);
        }
        return next;
      });
    };
    void getVehicles().then(merge);
    const close = subscribe((batch) => {
      setLive(true);
      merge(batch);
    });
    return () => {
      cancelled = true;
      close();
    };
  }, []);

  return { vehicles: [...byId.values()], live };
}

/** The map, and the one source every update writes to. */
export function VehicleMap({ collection }: { collection: VehicleCollection }) {
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!container.current || map.current) {
      return;
    }
    const instance = new MapLibreMap({
      container: container.current,
      style: STYLE,
      center: HELSINKI,
      zoom: 11,
    });
    map.current = instance;
    instance.on("load", () => {
      instance.addSource(SOURCE, { type: "geojson", data: collection });
      instance.addLayer({
        id: "vehicle-dots",
        type: "circle",
        source: SOURCE,
        paint: {
          "circle-radius": 7,
          "circle-color": ["get", "color"],
          "circle-stroke-width": 2,
          "circle-stroke-color": "#ffffff",
        },
      });
      setReady(true);
    });
    return () => {
      instance.remove();
      map.current = null;
    };
    // The collection is deliberately not a dependency: the map is built once and every later
    // fleet reaches it through setData below, which is what keeps a moving bus from
    // rebuilding the layer on every frame.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const source = map.current?.getSource(SOURCE) as GeoJSONSource | undefined;
    source?.setData(collection);
  }, [collection, ready]);

  return <div className="map" ref={container} data-testid="map" role="application" aria-label="Bus map" />;
}

export function App() {
  const { vehicles, live } = useVehicles();
  const collection = useMemo(() => featureCollection(vehicles), [vehicles]);
  const lines = useMemo(
    () => [...new Set(vehicles.map((vehicle) => vehicle.refLine).filter(Boolean))].sort() as string[],
    [vehicles],
  );

  return (
    <div className="app">
      <header>
        <h1>Buses live</h1>
        <span className="status" role="status">
          {vehicles.length === 0
            ? "Waiting for the first positions"
            : `${vehicles.length} buses${live ? ", updating live" : ""}`}
        </span>
      </header>
      <VehicleMap collection={collection} />
      {lines.length > 0 && (
        <div className="lines">
          {lines.map((line) => (
            <span className="line" key={line}>
              <span className="swatch" style={{ background: lineColor(line) }} />
              {line}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
