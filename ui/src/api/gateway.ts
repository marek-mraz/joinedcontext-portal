import { unwrap } from "./client";

export interface GatewayEntity {
  id: string;
  type: string;
  name?: string;
}

export async function searchEntities(params: {
  space: string;
  type: string;
  q?: string;
  limit?: number;
  signal?: AbortSignal;
}): Promise<GatewayEntity[]> {
  const query = new URLSearchParams();
  query.set("type", params.type);
  query.set("limit", String(params.limit ?? 20));

  const trimmedQ = params.q?.trim();
  if (trimmedQ) {
    // NGSI-LD quotes the pattern, so a quote or backslash in the term would end it early.
    const escaped = trimmedQ.replace(/[\\"]/g, (c) => `\\${c}`);
    query.set("q", `name~="${escaped}"`);
  }

  const url = `/cs/${encodeURIComponent(params.space)}/ngsi-ld/v1/entities?${query.toString()}`;

  const response = await fetch(url, {
    method: "GET",
    credentials: "same-origin",
    headers: {
      Accept: "application/ld+json",
    },
    signal: params.signal,
  });

  if (!response.ok) {
    const error = await response.json().catch(() => undefined);
    await unwrap({ error, response });
  }

  const data: unknown = await response.json();
  if (!Array.isArray(data)) {
    return [];
  }

  return data
    .filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null)
    .map((entity) => {
      const nameVal = (entity.name as { value?: unknown } | undefined)?.value;
      return {
        id: typeof entity.id === "string" ? entity.id : "",
        type: typeof entity.type === "string" ? entity.type : "",
        name: typeof nameVal === "string" ? nameVal : undefined,
      };
    })
    .filter((entity) => entity.id !== "");
}
