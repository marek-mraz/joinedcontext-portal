import type { components } from "./schema";

/**
 * A manifest as the views read it. `spec` is free-form JSON, which the OpenAPI document
 * declares as a bare object and openapi-typescript therefore renders `Record<string, never>`;
 * this is the one place that says what it really is (T-0249 re-derives it from jc-core).
 */
export type Manifest = Omit<components["schemas"]["ResourceEnvelope"], "spec"> & {
  spec: Record<string, unknown>;
};

export type Change = components["schemas"]["Change"];

/** A write answers `202` with a Change, or `200` with a dry-run result. */
export function isChange(result: unknown): result is Change {
  return typeof result === "object" && result !== null && "metadata" in result;
}

export function asManifests(items: components["schemas"]["ResourceEnvelope"][]): Manifest[] {
  return items as Manifest[];
}

/** Reads a language map (`metadata.title`) for the active locale, falling back to the name. */
export function localized(
  map: Record<string, string> | null | undefined,
  locale: string,
  fallback: string,
): string {
  if (!map) {
    return fallback;
  }
  return map[locale] ?? map[locale.split("-")[0]] ?? map.en ?? map.sk ?? fallback;
}
