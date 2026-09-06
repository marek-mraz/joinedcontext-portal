import createClient from "openapi-fetch";
import type { Middleware } from "openapi-fetch";
import type { components, paths } from "./schema";

export type ProblemDetails = components["schemas"]["ProblemDetails"];

export class ApiError extends Error {
  readonly status: number;
  readonly problem?: ProblemDetails;

  constructor(status: number, message: string, problem?: ProblemDetails) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.problem = problem;
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

export function readCsrfToken(): string | undefined {
  if (typeof document === "undefined" || !document.cookie) {
    return undefined;
  }
  const cookies = document.cookie.split("; ");
  for (const cookie of cookies) {
    const eqIdx = cookie.indexOf("=");
    if (eqIdx === -1) {
      continue;
    }
    const name = cookie.slice(0, eqIdx);
    if (name === "jc_csrf") {
      const rawValue = cookie.slice(eqIdx + 1);
      try {
        return decodeURIComponent(rawValue);
      } catch {
        return rawValue;
      }
    }
  }
  return undefined;
}

// openapi-fetch builds a `Request`, which needs an absolute URL outside the browser's
// document context (jsdom included). The paths in `schema.d.ts` already carry `/api/v1`.
export const api = createClient<paths>({
  baseUrl: window.location.origin,
  credentials: "same-origin",
  // Resolve the global at call time. openapi-fetch would otherwise capture whatever
  // `globalThis.fetch` was when this module first loaded, which no test can substitute.
  fetch: (request) => globalThis.fetch(request),
});

/** Echoes the double-submit cookie the backend sets non-HttpOnly for exactly this purpose. */
export const csrfMiddleware: Middleware = {
  onRequest({ request }) {
    const method = request.method.toUpperCase();
    if (method !== "GET" && method !== "HEAD" && method !== "OPTIONS") {
      const token = readCsrfToken();
      if (token) {
        request.headers.set("x-csrf-token", token);
      }
    }
    return request;
  },
};

api.use(csrfMiddleware);

export async function unwrap<T>(result: {
  data?: T;
  error?: unknown;
  response: Response;
}): Promise<T> {
  if (result.data !== undefined) {
    return result.data;
  }

  let problem: ProblemDetails | undefined;
  if (
    typeof result.error === "object" &&
    result.error !== null &&
    ("title" in result.error ||
      "detail" in result.error ||
      "status" in result.error ||
      "type" in result.error)
  ) {
    problem = result.error as ProblemDetails;
  }

  const status =
    typeof problem?.status === "number" ? problem.status : result.response.status;
  const message =
    problem?.detail ??
    problem?.title ??
    (result.response.statusText ? result.response.statusText : `HTTP ${status}`);

  throw new ApiError(status, message, problem);
}

export const queryKeys = {
  session: () => ["session"] as const,
  list: (project: string, plural: string) => ["projects", project, plural] as const,
  resource: (project: string, plural: string, name: string) =>
    ["projects", project, plural, name] as const,
  changes: (project: string) => ["projects", project, "changes"] as const,
  change: (project: string, id: string) => ["projects", project, "changes", id] as const,
};
