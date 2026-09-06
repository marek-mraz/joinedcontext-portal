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
  style: "circle" | "line" | "fill";
  colorBy?: { property: string; domain?: [number, number]; palette?: string };
  sizeBy?: { property: string; range?: [number, number] };
  popupProperties?: string[];
}

export interface MapLibreViewProps {
  layers: MapLayer[];
  center?: [number, number];
  zoom?: number;
  label: string;
}

/** Banská Bystrica: the demo city, and a better first view than null island. */
const DEFAULT_CENTER: [number, number] = [19.146, 48.736];
const DEFAULT_ZOOM = 11;

/** Yellow→orange→red, the "YlOrRd" ramp the Layer manifests name, as four stops. */
const RAMP = ["#ffffb2", "#fecc5c", "#fd8d3c", "#e31a1c"];

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

/**
 * Native MapLibre vector rendering of Layer manifests (UI-20, UI-21). Datasets of 50k
 * features and up belong to the deck.gl overlay of T-0225, not here.
 */
export function MapLibreView({ layers, center, zoom, label }: MapLibreViewProps): JSX.Element {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [failed, setFailed] = useState(false);

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
    instance.on("load", () => {
      for (const layer of layers) {
        instance.addSource(layer.name, { type: "geojson", data: layer.url });
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
      }
    });

    return () => {
      instance.remove();
      map.current = null;
    };
    // Layers come from manifests: they change when the dashboard does, and then the map
    // is rebuilt from scratch rather than diffed source by source.
  }, [layers, center, zoom]);

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
