/**
 * The preview bridge (T-0679, SDK-18, AP-63): a request from the frame this page created, under
 * the run's endpoint, is performed with the reviewer's session and answered, a write only when the
 * run's data needs name its operation; anything else is refused without a request, and the
 * frame's error reports go to the run.
 */
import { describe, expect, it, vi } from "vitest";
import { grantedOperations, handleBridgeMessage, operationOf, previewErrorOf } from "../src/pages/apps/previewBridge";
import type { PreviewError } from "../src/pages/apps/previewBridge";

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const BASE = `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`;
const ID = encodeURIComponent("urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002");
const GRANTED = new Set(["queryEntity", "retrieveEntity", "updateAttrs"]);

function request(overrides: Record<string, unknown> = {}) {
  return { kind: "jc-request", id: 7, method: "PATCH", path: `${BASE}/${ID}/attrs`, body: { status: { type: "Property", value: "closed" } }, ...overrides };
}

function frame() {
  return { postMessage: vi.fn() } as unknown as Window & { postMessage: ReturnType<typeof vi.fn> };
}

async function send(data: unknown, options: { source?: Window; from?: unknown; slug?: string; fetchImpl?: unknown; report?: (e: PreviewError) => void } = {}) {
  const source = options.source ?? frame();
  const fetchImpl = (options.fetchImpl ?? vi.fn()) as typeof fetch;
  const outcome = await handleBridgeMessage(
    { source: "from" in options ? options.from : source, data },
    { slug: "slug" in options ? options.slug : SLUG, operations: GRANTED, source },
    options.report ?? (() => undefined),
    fetchImpl,
  );
  return { outcome, source: source as Window & { postMessage: ReturnType<typeof vi.fn> }, fetchImpl: fetchImpl as unknown as ReturnType<typeof vi.fn> };
}

describe("operationOf", () => {
  it("names the reads and the three writes under the endpoint", () => {
    expect(operationOf("GET", `${BASE}?type=BikeHireDockingStation&limit=20`, SLUG)).toBe("read");
    expect(operationOf("GET", `${BASE}/${ID}`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/ngsi-ld/v1/temporal/entities?type=A&timerel=after&timeAt=2026-09-01T00%3A00%3A00Z`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/schema/index.json`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/schema/v3/json-schema`, SLUG)).toBe("read");
    expect(operationOf("GET", `/api/endpoint/${SLUG}/access`, SLUG)).toBe("read");
    expect(operationOf("POST", BASE, SLUG)).toBe("createEntity");
    expect(operationOf("PATCH", `${BASE}/${ID}/attrs`, SLUG)).toBe("updateAttrs");
    expect(operationOf("DELETE", `${BASE}/${ID}`, SLUG)).toBe("deleteEntity");
  });

  it("names nothing outside the endpoint, through a traversal, or with a query on a write", () => {
    for (const path of [
      `${BASE}/../../../../v1/projects/helsinki`,
      `${BASE}/%2e%2e/%2E%2E/attrs`,
      `${BASE}/..%2F..%2F..%2F..%2F..%2Fv1%2Fprojects%2Fhelsinki/attrs`,
      `${BASE}/a%252Fb/attrs`,
      `${BASE}/a%5Cb/attrs`,
      `${BASE}\\..\\admin`,
      `${BASE}//attrs`,
      `/api/endpoint/${SLUG}/ngsi-ld/v1/subscriptions`,
      `/api/endpoint/other26charslugother26cha/ngsi-ld/v1/entities`,
      `/api/v1/projects/helsinki/agent-runs`,
      `https://evil.example/api/endpoint/${SLUG}/access`,
      `//evil.example/api/endpoint/${SLUG}/access`,
      `${BASE}#x`,
    ]) {
      expect(operationOf("GET", path, SLUG), path).toBeNull();
      expect(operationOf("PATCH", path, SLUG), path).toBeNull();
    }
    expect(operationOf("POST", `${BASE}?options=upsert`, SLUG)).toBeNull();
    expect(operationOf("DELETE", `${BASE}/${ID}?purge=true`, SLUG)).toBeNull();
    expect(operationOf("PUT", `${BASE}/${ID}`, SLUG)).toBeNull();
    expect(operationOf("POST", `/api/endpoint/${SLUG}/ngsi-ld/v1/entityOperations/delete`, SLUG)).toBeNull();
    expect(operationOf("GET", BASE, "bikes")).toBeNull();
    expect(operationOf("GET", 42, SLUG)).toBeNull();
  });
});

describe("grantedOperations", () => {
  it("reads the operations of every data need and nothing else", () => {
    expect(grantedOperations([{ operations: ["queryEntity", "updateAttrs"] }, { operations: ["createEntity", 3] }])).toEqual(
      new Set(["queryEntity", "updateAttrs", "createEntity"]),
    );
    expect(grantedOperations(null)).toEqual(new Set());
    expect(grantedOperations([null, { operations: "updateAttrs" }])).toEqual(new Set());
  });
});

describe("handleBridgeMessage", () => {
  it("performs a read with the session and posts the answer back", async () => {
    const rows = [{ id: "urn:ngsi-ld:A:hel.fi:h:1", type: "A" }];
    const fetchImpl = vi.fn(async () => new Response(JSON.stringify(rows), { status: 200, headers: { "content-type": "application/ld+json" } }));
    const { outcome, source } = await send(request({ method: "GET", path: `${BASE}?type=A`, body: undefined }), { fetchImpl });
    expect(outcome).toBe("forwarded");
    const [path, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(`${BASE}?type=A`);
    expect(init.method).toBe("GET");
    expect(init.credentials).toBe("same-origin");
    expect(init.body).toBeUndefined();
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBeUndefined();
    expect(source.postMessage).toHaveBeenCalledWith({ kind: "jc-response", id: 7, status: 200, body: rows }, "*");
  });

  it("performs a granted write with the CSRF header and hands back the endpoint's answer", async () => {
    document.cookie = "jc_csrf=csrf-abc";
    const problem = { type: "about:blank", title: "Forbidden", status: 403, detail: "no policy grants this write" };
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(problem), { status: 403, headers: { "content-type": "application/problem+json" } }));
    const source = frame();
    expect((await send(request(), { source, fetchImpl })).outcome).toBe("forwarded");
    const [path, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(`${BASE}/${ID}/attrs`);
    expect(init.method).toBe("PATCH");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-abc");
    expect(JSON.parse(init.body as string)).toEqual({ status: { type: "Property", value: "closed" } });
    expect(source.postMessage).toHaveBeenLastCalledWith({ kind: "jc-response", id: 7, status: 204, body: null }, "*");

    await send(request({ id: 8 }), { source, fetchImpl });
    expect(source.postMessage).toHaveBeenLastCalledWith({ kind: "jc-response", id: 8, status: 403, body: problem }, "*");
  });

  it("answers a write the data needs do not name with 403 and sends no request", async () => {
    const { outcome, source, fetchImpl } = await send(request({ method: "DELETE", path: `${BASE}/${ID}`, body: undefined }));
    expect(outcome).toBe("refused");
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(source.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "jc-response", id: 7, status: 403, body: expect.objectContaining({ detail: "The run's data needs do not name deleteEntity." }) }),
      "*",
    );
  });

  it("refuses a traversal, another slug and a malformed write without a request", async () => {
    for (const data of [
      request({ path: `${BASE}/..%2F..%2F..%2F..%2F..%2Fv1%2Fprojects%2Fhelsinki/attrs` }),
      request({ path: `/api/endpoint/other26charslugother26cha/ngsi-ld/v1/entities/${ID}/attrs` }),
      request({ body: "text" }),
      request({ kind: "kit-write" }),
      request({ id: "7" }),
      null,
    ]) {
      const { outcome, fetchImpl } = await send(data);
      expect(outcome, JSON.stringify(data)).toBe("refused");
      expect(fetchImpl).not.toHaveBeenCalled();
    }
  });

  it("ignores another window, and every message when the run has no slug", async () => {
    const { outcome, source, fetchImpl } = await send(request(), { from: frame() });
    expect(outcome).toBe("ignored");
    expect((await send(request(), { slug: undefined })).outcome).toBe("ignored");
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(source.postMessage).not.toHaveBeenCalled();
  });

  it("relays the frame's error reports, bounded, and a foreign window's not at all", async () => {
    const report = vi.fn();
    const long = "x".repeat(3000);
    expect((await send({ kind: "jc-error", message: long, file: "src/App.tsx", line: 12 }, { report })).outcome).toBe("relayed");
    expect(report).toHaveBeenCalledWith({ message: "x".repeat(2000), file: "src/App.tsx", line: 12 });
    expect((await send({ kind: "jc-error", message: "boom" }, { report, from: frame() })).outcome).toBe("ignored");
    expect(report).toHaveBeenCalledTimes(1);
    expect(previewErrorOf({ kind: "jc-error", message: "boom", line: 0, file: 3 })).toEqual({ message: "boom" });
    expect(previewErrorOf({ kind: "jc-error", message: "  " })).toBeNull();
  });
});
