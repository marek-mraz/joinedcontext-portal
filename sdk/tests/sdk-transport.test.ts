import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { bridgeTransport, CSRF_COOKIE, CSRF_HEADER, originTransport, transportFor } from "../src/sdk/transport";

describe("originTransport", () => {
  it("includes CSRF header and content-type on PATCH, and omits them on GET", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const fakeFetch = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      calls.push({ url: String(url), init });
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }) as unknown as typeof fetch;

    const fakeDoc = { cookie: `${CSRF_COOKIE}=token-abc123; other=val` } as Document;
    const transport = originTransport(fakeFetch, fakeDoc);

    await transport({ method: "GET", path: "/api/endpoint/demo/access" });
    expect(calls[0].init?.headers).toEqual({ Accept: "application/json" });
    expect(calls[0].init?.body).toBeUndefined();

    await transport({ method: "PATCH", path: "/api/endpoint/demo/ngsi-ld/v1/entities/x/attrs", body: { a: 1 } });
    const patchHeaders = calls[1].init?.headers as Record<string, string>;
    expect(patchHeaders.Accept).toBe("application/json");
    expect(patchHeaders[CSRF_HEADER]).toBe("token-abc123");
    expect(patchHeaders["content-type"]).toBe("application/json");
    expect(calls[1].init?.body).toBe(JSON.stringify({ a: 1 }));
  });

  it("returns null body on 204, parses text when not json, and resolves status 0 on network error", async () => {
    const fakeFetch = vi.fn(async (url: string | URL | Request) => {
      const u = String(url);
      if (u.endsWith("/no-content")) {
        return new Response(null, { status: 204 });
      }
      if (u.endsWith("/plain-text")) {
        return new Response("hello plain text", { status: 200, headers: { "content-type": "text/plain" } });
      }
      throw new TypeError("Failed to fetch");
    }) as unknown as typeof fetch;

    const transport = originTransport(fakeFetch);

    const r1 = await transport({ method: "DELETE", path: "/no-content" });
    expect(r1.status).toBe(204);
    expect(r1.body).toBeNull();

    const r2 = await transport({ method: "GET", path: "/plain-text" });
    expect(r2.status).toBe(200);
    expect(r2.body).toBe("hello plain text");

    const r3 = await transport({ method: "GET", path: "/offline" });
    expect(r3.status).toBe(0);
    expect((r3.body as { title: string }).title).toContain("Failed to fetch");
  });
});

describe("bridgeTransport", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("posts jc-request, resolves only on matching id AND source, ignoring forged source", async () => {
    const target = {
      postMessage: vi.fn(),
    } as unknown as Window;

    let messageListener: ((ev: MessageEvent) => void) | undefined;
    const self = {
      addEventListener: vi.fn((_type: string, listener: (ev: MessageEvent) => void) => {
        messageListener = listener;
      }),
      removeEventListener: vi.fn(),
    } as unknown as Window;

    const transport = bridgeTransport({ target, self, timeoutMs: 5000 });
    const p = transport({ method: "GET", path: "/api/endpoint/demo/access" });

    expect(target.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "jc-request", id: 1, method: "GET", path: "/api/endpoint/demo/access" }),
      "*",
    );

    // Forged source ignored
    messageListener?.({
      source: {} as Window,
      data: { kind: "jc-response", id: 1, status: 200, body: { ok: true } },
    } as MessageEvent);

    // Wrong id ignored
    messageListener?.({
      source: target,
      data: { kind: "jc-response", id: 999, status: 200, body: { ok: true } },
    } as MessageEvent);

    // Matching source and id resolves
    messageListener?.({
      source: target,
      data: { kind: "jc-response", id: 1, status: 200, body: { ok: true } },
    } as MessageEvent);

    const res = await p;
    expect(res).toEqual({ status: 200, body: { ok: true } });
    expect(self.removeEventListener).toHaveBeenCalled();
  });

  it("times out to status 0 when framing page does not answer", async () => {
    const target = { postMessage: vi.fn() } as unknown as Window;
    const self = {
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    } as unknown as Window;

    const transport = bridgeTransport({ target, self, timeoutMs: 15000 });
    const p = transport({ method: "POST", path: "/functions/test" });

    vi.advanceTimersByTime(15000);
    const res = await p;
    expect(res.status).toBe(0);
    expect((res.body as { title: string }).title).toBe("The Portal did not answer.");
  });

  it("selects transport based on config.transport", () => {
    const origin = transportFor({
      slug: "demo",
      orgDomain: "example.org",
      space: "demo",
      transport: "origin",
      appName: "test-app",
    });
    expect(typeof origin).toBe("function");

    const bridge = transportFor({
      slug: "demo",
      orgDomain: "example.org",
      space: "demo",
      transport: "bridge",
    });
    expect(typeof bridge).toBe("function");
  });
});
