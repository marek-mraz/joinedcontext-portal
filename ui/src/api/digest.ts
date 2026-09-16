/**
 * The digest a verdict is fresh for: FNV-1a over canonical JSON, matching the backend's
 * `digest_of`. Its own module so a test (or anything outside the browser) can hash a manifest
 * without pulling in the API client and its `window`.
 */

/** Recursively sorts all object keys to ensure canonical serialization. */
export function canonicalizeJson(val: unknown): unknown {
  if (val === null || typeof val !== "object") {
    return val;
  }
  if (Array.isArray(val)) {
    return val.map(canonicalizeJson);
  }
  const sorted: Record<string, unknown> = {};
  for (const key of Object.keys(val as Record<string, unknown>).sort()) {
    const v = (val as Record<string, unknown>)[key];
    if (v !== undefined) {
      sorted[key] = canonicalizeJson(v);
    }
  }
  return sorted;
}

/** Computes 64-bit FNV-1a hex digest of canonical JSON without third-party dependencies. */
export function digestOf(value: unknown): string {
  const canonical = canonicalizeJson(value);
  const jsonStr = JSON.stringify(canonical);
  let hash = 0xcbf29ce484222325n;
  const prime = 0x100000001b3n;
  const mask = 0xffffffffffffffffn;
  const encoder = new TextEncoder();
  const bytes = encoder.encode(jsonStr);
  for (let i = 0; i < bytes.length; i++) {
    hash ^= BigInt(bytes[i]);
    hash = (hash * prime) & mask;
  }
  return hash.toString(16).padStart(16, "0");
}
