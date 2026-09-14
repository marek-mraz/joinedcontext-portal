import type { ComponentType, ReactNode } from "react";
import { Component, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { reportError } from "./report";
import { createClient, setClient } from "./client";
import type { Client } from "./client";
import { ConfigError, readConfig } from "./config";
import { JcProvider } from "./hooks";
import { applyTokens } from "./tokens";
import type { DesignTokens } from "./tokens";
import { transportFor } from "./transport";
import { startObserver } from "./observe";

let errorListenersRegistered = false;

/**
 * The last line under the application: a render error no component of the application caught is
 * reported and shown as its message. The template's own `ErrorBoundary` catches before this one.
 */
class RootBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  override state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error };
  }

  override componentDidCatch(error: Error): void {
    reportError(error);
  }

  override render(): ReactNode {
    return this.state.error ? <p role="alert">{this.state.error.message}</p> : this.props.children;
  }
}

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
        createRoot(targetRoot).render(<p role="alert">{err.message}</p>);
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
        <RootBoundary>
          <App />
        </RootBoundary>
      </JcProvider>
    </StrictMode>,
  );

  // A preview reads itself page by page once it has settled, so the run can check what it shows (SDK-27).
  if (config.transport === "bridge") {
    startObserver({ doc });
  }

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
