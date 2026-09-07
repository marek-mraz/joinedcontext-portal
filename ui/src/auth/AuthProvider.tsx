import { createContext, useCallback, useContext, useMemo } from "react";
import type { ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, queryKeys, readCsrfToken, unwrap } from "../api/client";
import type { components } from "../api/schema";

/** Who is signed in and through which front (`portal`, `edge` or `bearer`), from `/auth/me`. */
export type Identity = components["schemas"]["Me"];

/** The edge's own logout path: the APISIX `openid-connect` plugin ends its session and the
 * Keycloak one there, front-channel; the Portal never sees the request (ADR-N-019, AP-29). */
export const EDGE_LOGOUT_PATH = "/logout";

export type AuthStatus = "loading" | "authenticated" | "anonymous";

export interface AuthState {
  status: AuthStatus;
  identity: Identity | null;
  roles: string[];
  /** CC-42: display and enablement only — the API and the forge enforce the real boundary. */
  hasRole: (role: string) => boolean;
  /** Hands the browser to the server-side code flow; never returns. */
  signIn: (redirectTo?: string) => void;
  signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthState | null>(null);

async function fetchIdentity(): Promise<Identity | null> {
  const result = await api.GET("/api/v1/auth/me");
  // No session is the normal anonymous case, not a failure.
  if (result.response.status === 401) {
    return null;
  }
  return unwrap(result);
}

export function AuthProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const queryClient = useQueryClient();
  const session = useQuery({
    queryKey: queryKeys.session(),
    queryFn: fetchIdentity,
    retry: false,
    staleTime: 60_000,
  });

  const signIn = useCallback((redirectTo?: string) => {
    const target = redirectTo ?? `${window.location.pathname}${window.location.search}`;
    window.location.assign(`/api/v1/auth/login?redirect_to=${encodeURIComponent(target)}`);
  }, []);

  const signOut = useCallback(async () => {
    // The Portal's own cookies go first, whichever front signed the person in: behind the
    // edge they may still be there from an earlier code-flow login, and the edge's /logout
    // never reaches the Portal to clear them. A single logout is all three cookies and the
    // edge session (ADR-N-019, AP-29).
    const csrf = readCsrfToken();
    const response = await fetch("/api/v1/auth/logout", {
      method: "POST",
      credentials: "same-origin",
      headers: csrf ? { "x-csrf-token": csrf } : {},
    });
    queryClient.setQueryData(queryKeys.session(), null);
    if (session.data?.front === "edge") {
      // The login front is the plugin's, and so is the logout: it ends the edge session and
      // the Keycloak one, front-channel. Everything else about the UI is the same either way.
      window.location.assign(EDGE_LOGOUT_PATH);
      return;
    }
    const target: unknown = response.ok ? await response.json() : null;
    const endSessionUrl =
      typeof target === "object" && target !== null && "endSessionUrl" in target
        ? (target as { endSessionUrl: unknown }).endSessionUrl
        : undefined;
    window.location.assign(typeof endSessionUrl === "string" ? endSessionUrl : "/");
  }, [queryClient, session.data?.front]);

  const value = useMemo<AuthState>(() => {
    // Anything other than a live session is anonymous: a transport error must not open a door.
    const identity = session.data ?? null;
    const roles = identity?.roles ?? [];
    const status: AuthStatus = session.isPending
      ? "loading"
      : identity
        ? "authenticated"
        : "anonymous";
    return {
      status,
      identity,
      roles,
      hasRole: (role: string) => roles.includes(role),
      signIn,
      signOut,
    };
  }, [session.data, session.isPending, signIn, signOut]);

  return <AuthContext value={value}>{children}</AuthContext>;
}

export function useAuth(): AuthState {
  const value = useContext(AuthContext);
  if (!value) {
    throw new Error("useAuth must be used inside an AuthProvider");
  }
  return value;
}
