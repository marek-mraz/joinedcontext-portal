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

/**
 * The value without the empty leaves a form leaves behind: rjsf keeps an empty object for
 * every group the user opened and left alone, and an empty string for every field they
 * cleared, none of which a manifest should carry.
 */
export function prune<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.filter((item) => item !== undefined && item !== "") as T;
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, member] of Object.entries(value)) {
      const cleaned = prune(member);
      const empty =
        cleaned === undefined ||
        cleaned === "" ||
        (cleaned !== null && typeof cleaned === "object" && Object.keys(cleaned).length === 0);
      if (!empty) {
        out[key] = cleaned;
      }
    }
    return out as T;
  }
  return value;
}

/**
 * The name a `Ref` carries. jc-core writes a reference either as a bare name or as
 * `{ kind, name, namespace? }` (MF-07), and a manifest in the mirror may hold either, so
 * reading `spec.contextSpaceRef` as a string gives `[object Object]` half the time, or worse:
 * a manifest whose own reference is an object nested inside a reference.
 */
export function refName(reference: unknown): string {
  if (typeof reference === "string") {
    return reference;
  }
  if (typeof reference === "object" && reference !== null && "name" in reference) {
    const name = (reference as { name?: unknown }).name;
    return typeof name === "string" ? name : "";
  }
  return "";
}
