/**
 * The person's own copy of real data into a preview (PF-83, API/01 §22): read through the
 * origin's Endpoint, write through the preview's, both on the person's edge session, so each
 * Policy decides as it would for any other call. Bounded per type and in bytes.
 */

export const PER_TYPE = 1000;
export const MAX_BYTES = 10 * 1024 * 1024;
const PAGE = 100;

export interface CopyResult {
  /** Entities written, by type. */
  copied: Record<string, number>;
  /** Why the copy stopped early: a refusal, or the byte bound. */
  stopped?: string;
}

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

const URN = /^urn:ngsi-ld:([^:]+):([^:]+):([^:]+):(.+)$/;

/**
 * Moves every id of the entity's own organization to the preview's space segment, the way the
 * loader moves the ids a manifest writes. An id of another organization stays as it is.
 */
export function moveIds<T>(value: T, prefix: string, domain: string): T {
  if (typeof value === "string") {
    const m = URN.exec(value);
    if (m && m[2] === domain && !m[3].startsWith(prefix)) {
      return `urn:ngsi-ld:${m[1]}:${m[2]}:${prefix}${m[3]}:${m[4]}` as T;
    }
    return value;
  }
  if (Array.isArray(value)) return value.map((item) => moveIds(item, prefix, domain)) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, moveIds(item, prefix, domain)]),
    ) as T;
  }
  return value;
}

async function reason(response: Response): Promise<string> {
  const text = await response.text().catch(() => "");
  try {
    const problem = JSON.parse(text) as { detail?: string; title?: string };
    return problem.detail ?? problem.title ?? `HTTP ${response.status}`;
  } catch {
    return text || `HTTP ${response.status}`;
  }
}

export async function copyIntoPreview(
  originSlug: string,
  previewSlug: string,
  prefix: string,
  fetcher: Fetch = (input, init) => fetch(input, { credentials: "same-origin", ...init }),
): Promise<CopyResult> {
  const origin = `/api/endpoint/${originSlug}/ngsi-ld/v1`;
  const preview = `/api/endpoint/${previewSlug}/ngsi-ld/v1`;
  const json = { Accept: "application/json" };
  const result: CopyResult = { copied: {} };

  const types = await fetcher(`${origin}/types`, { headers: json });
  if (!types.ok) return { ...result, stopped: await reason(types) };
  const typeList = ((await types.json()) as { typeList?: string[] }).typeList ?? [];

  let bytes = 0;
  for (const type of typeList) {
    for (let offset = 0; offset < PER_TYPE; offset += PAGE) {
      const limit = Math.min(PAGE, PER_TYPE - offset);
      const query = new URLSearchParams({ type, limit: String(limit), offset: String(offset) });
      const page = await fetcher(`${origin}/entities?${query}`, { headers: json });
      if (!page.ok) return { ...result, stopped: await reason(page) };
      const entities = (await page.json()) as { id: string }[];
      if (entities.length === 0) break;
      const moved = entities.map((entity) => {
        const domain = URN.exec(entity.id)?.[2] ?? "";
        return moveIds(entity, prefix, domain);
      });
      const body = JSON.stringify(moved);
      bytes += body.length;
      if (bytes > MAX_BYTES) return { ...result, stopped: `the copy stops at ${MAX_BYTES / 1024 / 1024} MiB` };
      const written = await fetcher(`${preview}/entityOperations/upsert`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body,
      });
      if (!written.ok) return { ...result, stopped: await reason(written) };
      if (written.status === 207) {
        const report = (await written.json().catch(() => ({}))) as { errors?: { error?: { detail?: string } }[] };
        const refused = report.errors ?? [];
        result.copied[type] = (result.copied[type] ?? 0) + moved.length - refused.length;
        if (refused.length > 0) return { ...result, stopped: refused[0].error?.detail ?? "some entities were refused" };
      } else {
        result.copied[type] = (result.copied[type] ?? 0) + moved.length;
      }
      if (entities.length < limit) break;
    }
  }
  return result;
}
