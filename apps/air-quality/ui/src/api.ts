/**
 * The app's own backend, and nothing else. The endpoint URL never reaches the browser: the
 * server holds it, forwards the user's token to it and answers with what came back (AP-04).
 */
const BASE = import.meta.env.BASE_URL;

export interface Identity {
  signedIn: boolean;
  email: string | null;
  user: string | null;
  anonymous: boolean;
  canWriteNote: boolean;
}

export interface Station {
  id: string;
  name?: string;
  pm10?: number;
  pm25?: number;
  airQualityIndex?: number;
  stewardNote?: string;
  observedAt?: string;
  coordinates?: [number, number];
}

/** A refusal carries the gateway's own words, which is what the user needs to read. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly detail: string,
  ) {
    super(detail);
  }
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE}api/${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", ...(init?.headers ?? {}) },
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { detail?: string } | null;
    throw new ApiError(response.status, problem?.detail ?? response.statusText);
  }
  return response.status === 204 ? (undefined as T) : ((await response.json()) as T);
}

export const getIdentity = (): Promise<Identity> => call<Identity>("me");
export const getStations = (): Promise<Station[]> => call<Station[]>("stations");
export const writeNote = (id: string, note: string): Promise<void> =>
  call<void>(`stations/${encodeURIComponent(id)}/note`, {
    method: "POST",
    body: JSON.stringify({ note }),
  });
