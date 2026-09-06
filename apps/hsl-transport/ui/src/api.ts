/**
 * The app's own backend, and nothing else. The Endpoint URL never reaches the browser: the
 * server holds it, polls it once for every viewer and pushes what moved down this stream
 * (AP-04, AP-41). There is no login anywhere in this file, because the Endpoint behind the
 * app is public and the app never sees a user (AP-28).
 */
const BASE = import.meta.env.BASE_URL;

export interface Vehicle {
  id: string;
  /** `[longitude, latitude]`, the GeoJSON order. */
  coordinates: [number, number];
  bearing?: number;
  speed?: number;
  refLine?: string;
}

export interface VehicleFeature {
  type: "Feature";
  id: string;
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { id: string; color: string; bearing: number; refLine: string; speed?: number };
}

export interface VehicleCollection {
  type: "FeatureCollection";
  features: VehicleFeature[];
}

/** The fleet as the backend last saw it. A 503 means no poll has produced one yet. */
export async function getVehicles(): Promise<Vehicle[]> {
  const response = await fetch(`${BASE}api/vehicles`);
  if (!response.ok) {
    return [];
  }
  return (await response.json()) as Vehicle[];
}

/**
 * Every bus that moves. The first message carries the whole fleet, so a browser that opens
 * the stream alone is already drawing; returns the function that closes it.
 */
export function subscribe(onVehicles: (vehicles: Vehicle[]) => void): () => void {
  const source = new EventSource(`${BASE}api/stream`);
  source.addEventListener("vehicles", (event) => {
    try {
      onVehicles(JSON.parse((event as MessageEvent<string>).data) as Vehicle[]);
    } catch {
      // A malformed frame is one lost update, not a reason to tear the map down.
    }
  });
  return () => source.close();
}

/**
 * One colour per line, so two buses on the 550 look like the same route without anybody
 * maintaining a colour table. A line the Policy hides falls back to one neutral colour.
 */
export function lineColor(refLine: string | undefined): string {
  if (!refLine) {
    return "#64748b";
  }
  let hash = 0;
  for (const character of refLine) {
    hash = (hash * 31 + character.charCodeAt(0)) % 360;
  }
  return `hsl(${hash}, 72%, 42%)`;
}

/** The fleet as the map source reads it. */
export function featureCollection(vehicles: Vehicle[]): VehicleCollection {
  return {
    type: "FeatureCollection",
    features: vehicles.map((vehicle) => ({
      type: "Feature",
      id: vehicle.id,
      geometry: { type: "Point", coordinates: vehicle.coordinates },
      properties: {
        id: vehicle.id,
        color: lineColor(vehicle.refLine),
        // The arrow needs a number; a hidden bearing points north rather than disappearing.
        bearing: vehicle.bearing ?? 0,
        refLine: vehicle.refLine ?? "",
        ...(vehicle.speed === undefined ? {} : { speed: vehicle.speed }),
      },
    })),
  };
}
