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

/** The project segment of the organization's own manifests: roles, bindings, agent profiles, projects (PF-49). */
export const ORG_NAMESPACE = "org";

/** A write answers `202` with a Change, or `200` with a dry-run result. */
export function isChange(result: unknown): result is Change {
  return typeof result === "object" && result !== null && "metadata" in result;
}

export function asManifests(items: components["schemas"]["ResourceEnvelope"][]): Manifest[] {
  return items as Manifest[];
}

/**
 * Reads `metadata.title` for the active locale, falling back to the name. A title is one plain
 * string (UI-50); the legacy language map still reads.
 */
export function localized(
  title: string | Record<string, string> | null | undefined,
  locale: string,
  fallback: string,
): string {
  if (typeof title === "string") {
    return title.trim() === "" ? fallback : title;
  }
  if (!title) {
    return fallback;
  }
  return title[locale] ?? title[locale.split("-")[0]] ?? title.en ?? title.sk ?? fallback;
}

/**
 * A title as the one string a form edits and a manifest now writes (UI-50): a plain string as
 * written, a legacy language map as `en`, then its first non-empty value. Saving the form writes
 * the string, so a manifest converts itself on its next change.
 */
export function plainTitle(title: unknown): string | undefined {
  if (typeof title === "string") {
    return title.trim() === "" ? undefined : title;
  }
  if (!title || typeof title !== "object") {
    return undefined;
  }
  const texts = Object.entries(title as Record<string, unknown>).filter(
    (entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].trim() !== "",
  );
  return (texts.find(([locale]) => locale === "en") ?? texts[0])?.[1];
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
