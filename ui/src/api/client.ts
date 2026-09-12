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

/** The login page with the interrupted location to come back to; same shape as the server's. */
export function loginRedirectUrl(pathAndSearch: string): string {
  return `/login?redirect_to=${encodeURIComponent(pathAndSearch)}`;
}

/**
 * A 401 on any API call means the session is over (the server refreshes a live one itself):
 * the browser goes to the login page with the current location, so no page is left rendering
 * an empty list. `/auth/me` is exempt — a 401 there is the normal anonymous answer the
 * AuthProvider turns into the login redirect through the router. A 403 passes through
 * untouched and the page shows its forbidden state.
 */
export function createSessionMiddleware(navigate: (url: string) => void): Middleware {
  return {
    onResponse({ request, response }) {
      if (response.status !== 401) {
        return response;
      }
      const path = new URL(request.url, window.location.origin).pathname;
      if (path === "/api/v1/auth/me" || window.location.pathname === "/login") {
        return response;
      }
      navigate(loginRedirectUrl(`${window.location.pathname}${window.location.search}`));
      return response;
    },
  };
}

export const sessionMiddleware = createSessionMiddleware((url) => {
  window.location.assign(url);
});

api.use(sessionMiddleware);

export async function unwrap<T>(result: {
  data?: T;
  error?: unknown;
  response: Response;
}): Promise<T> {
  if (result.data !== undefined) {
    return result.data;
  }

  // A route that answers `204 No Content` has nothing to hand back and has not failed: an
  // answer accepted, a message sent. Reading the empty body as an error is what put
  // "No Content" on the run page in red.
  if (result.error === undefined && result.response.ok) {
    return undefined as T;
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
  // Not a prefix of `list`: invalidating one project's lists must not refetch the project list.
  projects: () => ["projectList"] as const,
  list: (project: string, plural: string) => ["projects", project, plural] as const,
  // Blueprints are organization-level, so they are not under a project key (CC-30).
  blueprints: () => ["blueprints"] as const,
  resource: (project: string, plural: string, name: string) =>
    ["projects", project, plural, name] as const,
  changes: (project: string) => ["projects", project, "changes"] as const,
  permissions: (project: string) => ["projects", project, "permissions"] as const,
  change: (project: string, id: string) => ["projects", project, "changes", id] as const,
};
