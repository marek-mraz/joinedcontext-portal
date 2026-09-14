import type { JcConfig } from "./config";

export type Method = "GET" | "POST" | "PATCH" | "DELETE";

export interface JcRequest {
  method: Method;
  path: string;
  body?: unknown;
}

export interface JcResponse {
  status: number;
  body: unknown;
}

export type Transport = (request: JcRequest) => Promise<JcResponse>;

export const CSRF_COOKIE = "jc_csrf";
export const CSRF_HEADER = "x-csrf-token";

function getCookie(doc: Document | undefined, name: string): string {
  if (!doc || !doc.cookie) return "";
  const found = doc.cookie.split("; ").find((c) => c.startsWith(`${name}=`));
  return found ? decodeURIComponent(found.slice(name.length + 1)) : "";
}

export function originTransport(fetchImpl?: typeof fetch, doc?: Document): Transport {
  const doFetch = fetchImpl ?? (typeof fetch !== "undefined" ? fetch.bind(globalThis) : undefined);
  return async (request: JcRequest): Promise<JcResponse> => {
    if (!doFetch) {
      return { status: 0, body: { title: "Fetch is not available in current environment." } };
    }
    const headers: Record<string, string> = {
      Accept: "application/json",
    };

    if (request.method !== "GET") {
      const token = getCookie(doc ?? (typeof document !== "undefined" ? document : undefined), CSRF_COOKIE);
      if (token) {
        headers[CSRF_HEADER] = token;
      }
      if (request.body !== undefined) {
        headers["content-type"] = "application/json";
      }
    }

    try {
      const response = await doFetch(request.path, {
        method: request.method,
        credentials: "same-origin",
        headers,
        body: request.body !== undefined && request.method !== "GET" ? JSON.stringify(request.body) : undefined,
      });

      if (response.status === 204) {
        return { status: 204, body: null };
      }

      const contentType = response.headers.get("content-type") ?? "";
      let body: unknown = null;
      if (contentType.includes("json")) {
        body = await response.json().catch(() => null);
      } else {
        const text = await response.text().catch(() => "");
        body = text === "" ? null : text;
      }
      return { status: response.status, body };
    } catch (err) {
      return {
        status: 0,
        body: { title: err instanceof Error ? err.message : String(err) },
      };
    }
  };
}

export interface BridgeOptions {
  target?: Window;
  self?: Window;
  timeoutMs?: number;
}

export function bridgeTransport(options?: BridgeOptions): Transport {
  let seq = 0;
  return (request: JcRequest): Promise<JcResponse> => {
    const target = options?.target ?? (typeof window !== "undefined" ? window.parent : undefined);
    const self = options?.self ?? (typeof window !== "undefined" ? window : undefined);
    const timeoutMs = options?.timeoutMs ?? 15000;
    const id = ++seq;

    return new Promise((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;

      const cleanup = () => {
        if (timer) clearTimeout(timer);
        self?.removeEventListener("message", onMessage);
      };

      const onMessage = (event: MessageEvent) => {
        if (event.source !== target) return;
        const data = event.data as Partial<{ kind: string; id: number; status: number; body: unknown }> | null;
        if (!data || data.kind !== "jc-response" || data.id !== id || typeof data.status !== "number") return;
        cleanup();
        resolve({ status: data.status, body: data.body });
      };

      self?.addEventListener("message", onMessage);

      timer = setTimeout(() => {
        cleanup();
        resolve({ status: 0, body: { title: "The Portal did not answer." } });
      }, timeoutMs);

      target?.postMessage(
        {
          kind: "jc-request",
          id,
          method: request.method,
          path: request.path,
          body: request.body,
        },
        "*",
      );
    });
  };
}

export function transportFor(config: JcConfig): Transport {
  return config.transport === "origin" ? originTransport() : bridgeTransport();
}
