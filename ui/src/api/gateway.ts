import { ApiError } from "./client";
import type { ProblemDetails } from "./client";

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
    let problem: ProblemDetails | undefined;
    try {
      const body: unknown = await response.json();
      if (
        typeof body === "object" &&
        body !== null &&
        ("title" in body || "detail" in body || "status" in body || "type" in body)
      ) {
        problem = body as ProblemDetails;
      }
    } catch {
      // Body not JSON
    }
    const status = problem?.status ?? response.status;
    const message =
      problem?.detail ??
      problem?.title ??
      (response.statusText ? response.statusText : `HTTP ${status}`);
    throw new ApiError(status, message, problem);
  }

  const data: unknown = await response.json();
  if (!Array.isArray(data)) {
    return [];
  }

  return data
    .filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null)
    .map((entity) => {
      const id = typeof entity.id === "string" ? entity.id : "";
      const type = typeof entity.type === "string" ? entity.type : "";
      let name: string | undefined;
      if (
        typeof entity.name === "object" &&
        entity.name !== null &&
        "value" in entity.name &&
        typeof (entity.name as { value: unknown }).value === "string"
      ) {
        name = (entity.name as { value: string }).value;
      }
      return { id, type, name };
    })
    .filter((entity) => entity.id !== "");
}
