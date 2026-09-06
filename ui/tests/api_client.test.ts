import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, csrfMiddleware, queryKeys, readCsrfToken, unwrap } from "../src/api/client";
import type { ProblemDetails } from "../src/api/client";
import { searchEntities } from "../src/api/gateway";

describe("readCsrfToken", () => {
  beforeEach(() => {
    // Clear cookies in jsdom
    for (const cookie of document.cookie.split(";")) {
      const eqIdx = cookie.indexOf("=");
      const name = eqIdx > -1 ? cookie.slice(0, eqIdx).trim() : cookie.trim();
      if (name) {
        document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/`;
      }
    }
  });

  it("returns the value from document.cookie when multiple cookies are present", () => {
    document.cookie = "session_id=xyz123; path=/";
    document.cookie = "jc_csrf=test-csrf-token; path=/";
    document.cookie = "pref=dark; path=/";

    expect(readCsrfToken()).toBe("test-csrf-token");
  });

  it("returns undefined when jc_csrf cookie is absent", () => {
    document.cookie = "session_id=xyz123; path=/";
    document.cookie = "pref=dark; path=/";

    expect(readCsrfToken()).toBeUndefined();
  });

  it("decodes URI encoded cookie value", () => {
    document.cookie = `jc_csrf=${encodeURIComponent("token/with+special=chars")}; path=/`;

    expect(readCsrfToken()).toBe("token/with+special=chars");
  });
});

describe("csrfMiddleware", () => {
  beforeEach(() => {
    for (const cookie of document.cookie.split(";")) {
      const eqIdx = cookie.indexOf("=");
      const name = eqIdx > -1 ? cookie.slice(0, eqIdx).trim() : cookie.trim();
      if (name) {
        document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/`;
      }
    }
  });

  async function run(method: string): Promise<Request> {
    const request = new Request(`${window.location.origin}/api/v1/health`, { method });
    const onRequest = csrfMiddleware.onRequest;
    if (!onRequest) {
      throw new Error("csrfMiddleware has no onRequest hook");
    }
    const result = await onRequest({ request } as Parameters<typeof onRequest>[0]);
    return result instanceof Request ? result : request;
  }

  it("does not add x-csrf-token to a GET request", async () => {
    document.cookie = "jc_csrf=my-token; path=/";

    const request = await run("GET");
    expect(request.headers.get("x-csrf-token")).toBeNull();
  });

  it("adds x-csrf-token to a POST request when the jc_csrf cookie is present", async () => {
    document.cookie = "jc_csrf=my-token; path=/";

    const request = await run("POST");
    expect(request.headers.get("x-csrf-token")).toBe("my-token");
  });

  it("leaves a POST unchanged when the jc_csrf cookie is absent", async () => {
    const request = await run("POST");
    expect(request.headers.get("x-csrf-token")).toBeNull();
  });

  it("covers every unsafe method", async () => {
    document.cookie = "jc_csrf=my-token; path=/";

    for (const method of ["POST", "PUT", "PATCH", "DELETE"]) {
      const request = await run(method);
      expect(request.headers.get("x-csrf-token"), method).toBe("my-token");
    }
    for (const method of ["GET", "HEAD", "OPTIONS"]) {
      const request = await run(method);
      expect(request.headers.get("x-csrf-token"), method).toBeNull();
    }
  });
});

describe("unwrap", () => {
  it("returns data on success", async () => {
    const mockData = { name: "test-resource" };
    const response = new Response(JSON.stringify(mockData), { status: 200 });

    const result = await unwrap({ data: mockData, response });
    expect(result).toEqual(mockData);
  });

  it("throws ApiError carrying status and detail on an RFC 7807 problem body", async () => {
    const problem: ProblemDetails = {
      type: "https://example.com/errors/not-found",
      title: "Not Found",
      status: 404,
      detail: "The requested project does not exist.",
    };
    const response = new Response(JSON.stringify(problem), { status: 404 });

    await expect(unwrap({ error: problem, response })).rejects.toThrow(ApiError);

    try {
      await unwrap({ error: problem, response });
    } catch (err: unknown) {
      expect(err).toBeInstanceOf(ApiError);
      const apiError = err as ApiError;
      expect(apiError.status).toBe(404);
      expect(apiError.problem).toEqual(problem);
      expect(apiError.problem?.detail).toBe("The requested project does not exist.");
      expect(apiError.message).toBe("The requested project does not exist.");
    }
  });

  it("throws ApiError with fallback status text when problem details are omitted", async () => {
    const response = new Response(null, { status: 500, statusText: "Internal Server Error" });

    try {
      await unwrap({ response });
    } catch (err: unknown) {
      expect(err).toBeInstanceOf(ApiError);
      const apiError = err as ApiError;
      expect(apiError.status).toBe(500);
      expect(apiError.message).toBe("Internal Server Error");
    }
  });
});

describe("queryKeys", () => {
  it("returns stable query key tuples", () => {
    expect(queryKeys.session()).toEqual(["session"]);
    expect(queryKeys.list("default", "workspaces")).toEqual(["projects", "default", "workspaces"]);
    expect(queryKeys.resource("default", "workspaces", "ws-1")).toEqual([
      "projects",
      "default",
      "workspaces",
      "ws-1",
    ]);
  });
});

describe("searchEntities", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("builds correct URL and headers and maps entity properties", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify([
          {
            id: "urn:ngsi-ld:Sensor:01",
            type: "Sensor",
            name: { type: "Property", value: "Temp Sensor" },
          },
          {
            id: "urn:ngsi-ld:Sensor:02",
            type: "Sensor",
          },
        ]),
        { status: 200, headers: { "Content-Type": "application/ld+json" } }
      )
    );

    const entities = await searchEntities({
      space: "city-iot",
      type: "Sensor",
      q: "Temp",
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(decodeURIComponent(url)).toContain(
      '/cs/city-iot/ngsi-ld/v1/entities?type=Sensor&limit=20&q=name~="Temp"'
    );
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>)?.Accept).toBe("application/ld+json");

    expect(entities).toEqual([
      { id: "urn:ngsi-ld:Sensor:01", type: "Sensor", name: "Temp Sensor" },
      { id: "urn:ngsi-ld:Sensor:02", type: "Sensor", name: undefined },
    ]);
  });

  it("throws ApiError on non-2xx response", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          type: "about:blank",
          title: "Forbidden",
          status: 403,
          detail: "Access denied to space",
        }),
        { status: 403, headers: { "Content-Type": "application/problem+json" } }
      )
    );

    await expect(
      searchEntities({
        space: "restricted",
        type: "Sensor",
      })
    ).rejects.toThrow(ApiError);
  });

  it("propagates AbortError untouched when aborted", async () => {
    const controller = new AbortController();
    controller.abort();

    fetchMock.mockRejectedValue(new DOMException("The operation was aborted.", "AbortError"));

    await expect(
      searchEntities({
        space: "space1",
        type: "Sensor",
        signal: controller.signal,
      })
    ).rejects.toMatchObject({ name: "AbortError" });
  });
});
