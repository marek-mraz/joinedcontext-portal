import { vi } from "vitest";

type Fetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

/**
 * Every page checks a manifest before it proposes it (PF-57, T-0956), so a test's fetch stub
 * sees a `?dryRun=All` request before each proposal. This answers those checks green and records
 * them (`METHOD /path`), and hands every other request to the test's own stub unchanged, so what
 * a test asserts about its proposals still counts proposals only.
 */
export function answeringChecks(inner: Fetch): Fetch & { checks: string[] } {
  const checks: string[] = [];
  const wrapped = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const url = new URL(request ? request.url : String(input), window.location.origin);
    if (url.searchParams.get("dryRun") === "All") {
      checks.push(`${request?.method ?? init?.method ?? "GET"} ${url.pathname}`);
      return new Response(JSON.stringify({ valid: true, verdict: { ok: true, findings: [] } }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    return inner(input, init);
  });
  return Object.assign(wrapped, { checks });
}

/** The checks the current `fetch` stub answered, when it was wrapped by `answeringChecks`. */
export function checksSoFar(): string[] {
  return (globalThis.fetch as unknown as { checks?: string[] }).checks ?? [];
}
