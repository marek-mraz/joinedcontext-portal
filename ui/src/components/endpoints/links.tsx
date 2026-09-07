import type { JSX } from "react";
import { Icon } from "../ui";

/**
 * Where each enabled representation answers under the endpoint's URL (EP-08, EP-44): the
 * same paths the gateway's DCAT index at `/api/endpoint/{slug}/` lists, so a steward can hand
 * out one link per shape without opening the index.
 */
export const REPRESENTATION_PATHS: Record<string, string> = {
  "ngsi-ld": "/ngsi-ld/v1/entities?limit=20",
  mcp: "/mcp",
  geojson: "/file.geojson",
  csv: "/file.csv",
  xlsx: "/file.xlsx",
  zip: "/file.zip",
  "ogc-features": "/ogc/features",
  sta: "/sta/v1.1",
};

/** The links every endpoint has whatever it enables: its index and the model it publishes. */
export const ENDPOINT_LINKS: Array<{ key: string; path: string }> = [
  { key: "index", path: "/" },
  { key: "linkml", path: "/schema/v1/linkml" },
  { key: "jsonSchema", path: "/schema/v1/json-schema" },
  { key: "access", path: "/access" },
];

export function endpointUrl(slug: string, path: string): string {
  return `${window.location.origin}/api/endpoint/${slug}${path}`;
}

/** The open-data catalogue entry of an endpoint: the catalogue lives at `data.{host}`. */
export function catalogueUrl(endpointName: string): string {
  return `https://data.${window.location.host}/dataset/${encodeURIComponent(endpointName)}`;
}

export function EndpointLink({ href, children }: { href: string; children: string }): JSX.Element {
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      title={href}
      className="focus-ring inline-flex items-center gap-1 rounded-full border border-primary-200 bg-primary-soft px-2 py-0.5 font-mono text-caption font-medium text-primary-soft-fg hover:border-primary-400 hover:bg-primary-100"
    >
      {children}
      <Icon name="external" className="size-3" />
    </a>
  );
}
