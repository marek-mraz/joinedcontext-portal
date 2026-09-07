import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { Map as MapLibreMap, NavigationControl, Popup } from "maplibre-gl";
import type { MapGeoJSONFeature } from "maplibre-gl";
import { useTranslation } from "react-i18next";
import "maplibre-gl/dist/maplibre-gl.css";

/** One Layer manifest, resolved against the Endpoint it reads from. */
export interface MapLayer {
  name: string;
  /** Absolute or same-origin GeoJSON URL of the Endpoint, `geoQ` and friends included. */
  url: string;
  /**
   * The features, when the caller has already fetched them to count them (UI-21). MapLibre
   * takes a source that is either a URL it fetches or the collection itself, and handing it
   * what is already in memory is what keeps a dashboard from downloading each layer twice.
   */
  data?: unknown;
  style: "circle" | "line" | "fill";
  colorBy?: { property: string; domain?: [number, number]; palette?: string };
  sizeBy?: { property: string; range?: [number, number] };
  popupProperties?: string[];
}

/** `[west, south, east, north]` of the viewport, as the Endpoint's `coordinates` wants it. */
export type Bbox = [number, number, number, number];

export interface MapLibreViewProps {
  layers: MapLayer[];
  center?: [number, number];
  zoom?: number;
  label: string;
  /** The viewport after every pan or zoom, for the page to ask the Endpoints again (UI-22). */
  onMoveEnd?: (bbox: Bbox) => void;
  /**
   * Called once the map exists, for an overlay to attach itself to it (UI-20); what it
   * returns is called when the map goes away. This is how the deck.gl overlay draws on the
   * same map instead of a second one beside it.
   */
  onReady?: (map: MapLibreMap) => (() => void) | void;
}

/** Banská Bystrica: the demo city, and a better first view than null island. */
const DEFAULT_CENTER: [number, number] = [19.146, 48.736];
const DEFAULT_ZOOM = 11;

/** Yellow→orange→red, the "YlOrRd" ramp the Layer manifests name, as four stops. */
export const RAMP = ["#ffffb2", "#fecc5c", "#fd8d3c", "#e31a1c"];

function paintFor(layer: MapLayer): Record<string, unknown> {
  const color = layer.colorBy
    ? [
        "interpolate",
        ["linear"],
        ["to-number", ["get", layer.colorBy.property], 0],
        ...RAMP.flatMap((stop, index) => {
          const [min, max] = layer.colorBy?.domain ?? [0, 100];
          return [min + ((max - min) * index) / (RAMP.length - 1), stop];
        }),
      ]
    : "#2563eb";

  if (layer.style === "line") {
    return { "line-color": color, "line-width": 2 };
  }
  if (layer.style === "fill") {
    return { "fill-color": color, "fill-opacity": 0.5, "fill-outline-color": "#1f2937" };
  }

  const [minRadius, maxRadius] = layer.sizeBy?.range ?? [5, 5];
  const radius = layer.sizeBy
    ? [
        "interpolate",
        ["linear"],
        ["to-number", ["get", layer.sizeBy.property], 0],
        layer.colorBy?.domain?.[0] ?? 0,
        minRadius,
        layer.colorBy?.domain?.[1] ?? 100,
        maxRadius,
      ]
    : minRadius;
  return { "circle-color": color, "circle-radius": radius, "circle-stroke-width": 1, "circle-stroke-color": "#1f2937" };
}

function popupHtml(feature: MapGeoJSONFeature, properties?: string[]): string {
  const props = feature.properties ?? {};
  const keys = properties?.length ? properties : Object.keys(props).slice(0, 6);
  const escape = (value: unknown) =>
    String(value ?? "—").replace(
      /[&<>"']/g,
      (char) =>
        ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char] ?? char,
    );
  return `<dl>${keys
    .map((key) => `<dt>${escape(key)}</dt><dd>${escape(props[key])}</dd>`)
    .join("")}</dl>`;
}

/** The extent of a feature collection, or nothing when it has no coordinate. */
export function extentOf(data: unknown): Bbox | null {
  let box: Bbox | null = null;
  const visit = (value: unknown) => {
    if (!Array.isArray(value)) {
      return;
    }
    if (value.length >= 2 && typeof value[0] === "number" && typeof value[1] === "number") {
      const [x, y] = value as number[];
      if (!Number.isFinite(x) || !Number.isFinite(y)) {
        return;
      }
      box = box
        ? [Math.min(box[0], x), Math.min(box[1], y), Math.max(box[2], x), Math.max(box[3], y)]
        : [x, y, x, y];
      return;
    }
    value.forEach(visit);
  };
  const features = (data as { features?: { geometry?: { coordinates?: unknown } }[] } | undefined)?.features;
  features?.forEach((feature) => visit(feature.geometry?.coordinates));
  return box;
}

/**
 * Native MapLibre vector rendering of Layer manifests (UI-20, UI-21). Datasets of 50k
 * features and up belong to the deck.gl overlay of T-0225, not here.
 */
export function MapLibreView({
  layers,
  center,
  zoom,
  label,
  onReady,
  onMoveEnd,
}: MapLibreViewProps): JSX.Element {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState<MapLibreMap | null>(null);
  // The latest callback, so a pan does not rebuild the map.
  const moveEnd = useRef(onMoveEnd);
  useEffect(() => {
    moveEnd.current = onMoveEnd;
  }, [onMoveEnd]);
  // The layers on the map, by name, to diff the next render against.
  const drawn = useRef(new Map<string, MapLayer>());
  const fitted = useRef(false);

  useEffect(() => {
    if (!container.current || map.current) {
      return;
    }
    let instance: MapLibreMap;
    try {
      instance = new MapLibreMap({
        container: container.current,
        style: import.meta.env.VITE_MAP_STYLE ?? "https://demotiles.maplibre.org/style.json",
        center: center ?? DEFAULT_CENTER,
        zoom: zoom ?? DEFAULT_ZOOM,
      });
      instance.addControl(new NavigationControl(), "top-right");
    } catch {
      // No WebGL (a locked-down browser, a headless runner) must not blank the page.
      // The state change is the whole point of the catch, so the lint rule is wrong here.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setFailed(true);
      return;
    }
    map.current = instance;
    const detach = onReady?.(instance);
    instance.on("load", () => setLoaded(instance));
    instance.on("moveend", () => {
      const bounds = instance.getBounds();
      moveEnd.current?.([bounds.getWest(), bounds.getSouth(), bounds.getEast(), bounds.getNorth()]);
    });

    return () => {
      detach?.();
      instance.remove();
      map.current = null;
      drawn.current.clear();
      setLoaded(null);
    };
  }, [center, zoom, onReady]);

  // The layers are diffed against what the map holds: a refetch after a pan replaces the
  // data of a source in place, so the viewport the reader chose survives it (UI-22).
  useEffect(() => {
    const instance = loaded;
    if (!instance) {
      return;
    }
    const wanted = new Set(layers.map((layer) => layer.name));
    for (const name of [...drawn.current.keys()]) {
      if (!wanted.has(name)) {
        instance.removeLayer(name);
        instance.removeSource(name);
        drawn.current.delete(name);
      }
    }
    for (const layer of layers) {
      const data = (layer.data ?? layer.url) as string;
      const before = drawn.current.get(layer.name);
      if (before) {
        if (before.data !== layer.data || before.url !== layer.url) {
          (instance.getSource(layer.name) as { setData?: (d: string) => void } | undefined)?.setData?.(data);
          drawn.current.set(layer.name, layer);
        }
        continue;
      }
      instance.addSource(layer.name, { type: "geojson", data });
      instance.addLayer({
        id: layer.name,
        type: layer.style,
        source: layer.name,
        paint: paintFor(layer),
      } as Parameters<MapLibreMap["addLayer"]>[0]);
      instance.on("click", layer.name, (event) => {
        const feature = event.features?.[0];
        if (!feature) {
          return;
        }
        new Popup()
          .setLngLat(event.lngLat)
          .setHTML(popupHtml(feature, layer.popupProperties))
          .addTo(instance);
      });
      drawn.current.set(layer.name, layer);
    }
    // The first data decides the view: a dashboard of Helsinki opens on Helsinki, not on the
    // default city, and only once, so a pan is never undone by a refetch.
    if (!fitted.current && !center) {
      const boxes = layers.map((layer) => extentOf(layer.data)).filter((box): box is Bbox => box !== null);
      if (boxes.length > 0) {
        const box = boxes.reduce((a, b) => [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])]);
        fitted.current = true;
        instance.fitBounds([box[0], box[1], box[2], box[3]], { padding: 40, maxZoom: 14, duration: 0 });
      }
    }
  }, [layers, loaded, center]);

  if (failed) {
    return (
      <div role="status" className="rounded border border-border bg-surface-subtle p-4 text-sm">
        {t("dashboards.mapUnavailable")}
      </div>
    );
  }

  return (
    <div
      ref={container}
      role="application"
      aria-label={label}
      className="h-[28rem] w-full overflow-hidden rounded border border-border"
    />
  );
}
