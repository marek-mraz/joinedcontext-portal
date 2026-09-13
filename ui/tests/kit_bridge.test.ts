/**
 * The preview's write bridge (T-0594, AP-63): a write from the frame this page created, for the
 * slug the preview was built for, under the endpoint's entity route, is performed with the
 * reviewer's session and answered; anything else is refused without a request.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { acceptsWrite, handleKitMessage } from "../src/pages/apps/kitBridge";

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const BASE = `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`;
const ID = encodeURIComponent("urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002");

function write(overrides: Record<string, unknown> = {}) {
  return { kind: "kit-write", id: 7, slug: SLUG, method: "PATCH", path: `${BASE}/${ID}/attrs`, body: { status: { type: "Property", value: "closed" } }, ...overrides };
}

function frame() {
  return { postMessage: vi.fn() } as unknown as Window;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("acceptsWrite", () => {
  it("takes the two operations under the endpoint and nothing else", () => {
    expect(acceptsWrite(write(), SLUG)).toBe(true);
    expect(acceptsWrite(write({ method: "POST", path: BASE }), SLUG)).toBe(true);
    expect(acceptsWrite(write({ slug: "other" }), SLUG)).toBe(false);
    expect(acceptsWrite(write(), "another2another2another2ab")).toBe(false);
    expect(acceptsWrite(write({ method: "DELETE" }), SLUG)).toBe(false);
    expect(acceptsWrite(write({ path: `${BASE}/${ID}` }), SLUG)).toBe(false);
    expect(acceptsWrite(write({ path: `${BASE}/../../v1/admin` }), SLUG)).toBe(false);
    expect(acceptsWrite(write({ path: `/api/endpoint/other26charslugother26cha/ngsi-ld/v1/entities` }), SLUG)).toBe(false);
    expect(acceptsWrite(write({ method: "POST", path: `${BASE}?x=1` }), SLUG)).toBe(false);
    expect(acceptsWrite(write({ body: "text" }), SLUG)).toBe(false);
    expect(acceptsWrite(write({ kind: "kit-read" }), SLUG)).toBe(false);
    expect(acceptsWrite(null, SLUG)).toBe(false);
  });
});

describe("handleKitMessage", () => {
  it("performs the write with the session and posts the answer back to the frame", async () => {
    document.cookie = "jc_csrf=csrf-abc";
    const fetchMock = vi.fn(async () => new Response(null, { status: 204 }));
    const source = frame();
    const outcome = await handleKitMessage({ source, data: write() }, SLUG, source, fetchMock as unknown as typeof fetch);
    expect(outcome).toBe("forwarded");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [path, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(`${BASE}/${ID}/attrs`);
    expect(init.method).toBe("PATCH");
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-abc");
    expect(JSON.parse(init.body as string)).toEqual({ status: { type: "Property", value: "closed" } });
    expect(source.postMessage).toHaveBeenCalledWith({ kind: "kit-write-result", id: 7, status: 204, body: null }, "*");
  });

  it("hands a refusal back as the problem the endpoint answered", async () => {
    const problem = { type: "about:blank", title: "Forbidden", status: 403, detail: "no policy grants this write" };
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(problem), { status: 403, headers: { "content-type": "application/problem+json" } }));
    const source = frame();
    await handleKitMessage({ source, data: write() }, SLUG, source, fetchMock as unknown as typeof fetch);
    expect(source.postMessage).toHaveBeenCalledWith({ kind: "kit-write-result", id: 7, status: 403, body: problem }, "*");
  });

  it("refuses what is not a write for this slug, and ignores another window, without a request", async () => {
    const fetchMock = vi.fn();
    const source = frame();
    expect(await handleKitMessage({ source, data: write({ slug: "other" }) }, SLUG, source, fetchMock as unknown as typeof fetch)).toBe("refused");
    expect(await handleKitMessage({ source, data: write({ method: "DELETE" }) }, SLUG, source, fetchMock as unknown as typeof fetch)).toBe("refused");
    expect(await handleKitMessage({ source: frame(), data: write() }, SLUG, source, fetchMock as unknown as typeof fetch)).toBe("ignored");
    expect(await handleKitMessage({ source, data: write() }, undefined, source, fetchMock as unknown as typeof fetch)).toBe("ignored");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(source.postMessage).not.toHaveBeenCalled();
  });
});
