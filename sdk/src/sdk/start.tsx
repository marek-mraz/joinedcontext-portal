import type { ComponentType } from "react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ErrorBoundary, Problem, reportError } from "./components/states";
import { createClient, setClient } from "./client";
import type { Client } from "./client";
import { ConfigError, readConfig } from "./config";
import { JcProvider } from "./hooks";
import { applyTokens } from "./tokens";
import type { DesignTokens } from "./tokens";
import { transportFor } from "./transport";

let errorListenersRegistered = false;

export function startApp(
  App: ComponentType,
  options?: { tokens?: unknown; root?: HTMLElement; doc?: Document },
): { client: Client; tokens: DesignTokens } {
  const doc = options?.doc ?? (typeof document !== "undefined" ? document : undefined);

  let config;
  try {
    config = readConfig(doc);
  } catch (err) {
    if (err instanceof ConfigError) {
      const targetRoot = options?.root ?? doc?.getElementById("root");
      if (targetRoot) {
        createRoot(targetRoot).render(<Problem error={err} />);
      }
    }
    throw err;
  }

  const client = createClient(config, transportFor(config));
  setClient(client);

  const tokens = applyTokens(options?.tokens ?? {});

  const targetRoot = options?.root ?? doc?.getElementById("root");
  if (!targetRoot) {
    throw new Error("No #root element");
  }

  createRoot(targetRoot).render(
    <StrictMode>
      <JcProvider client={client}>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </JcProvider>
    </StrictMode>,
  );

  if (!errorListenersRegistered && typeof window !== "undefined") {
    errorListenersRegistered = true;
    window.addEventListener("error", (e: ErrorEvent) => {
      reportError(e.error ?? e.message);
    });
    window.addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
      reportError(e.reason);
    });
  }

  return { client, tokens };
}
