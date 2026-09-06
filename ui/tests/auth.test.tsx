import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { AuthProvider, useAuth } from "../src/auth/AuthProvider";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer", "space-editor"],
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function problem(status: number): Response {
  return new Response(
    JSON.stringify({ type: "https://joinedcontext.com/errors/unauthorized", status }),
    { status, headers: { "Content-Type": "application/problem+json" } },
  );
}

function renderApp(client: QueryClient) {
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function Probe() {
  const { status, roles, hasRole } = useAuth();
  return (
    <ul>
      <li data-testid="status">{status}</li>
      <li data-testid="roles">{roles.join(",")}</li>
      <li data-testid="editor">{hasRole("space-editor") ? "yes" : "no"}</li>
      <li data-testid="admin">{hasRole("city-admin") ? "yes" : "no"}</li>
    </ul>
  );
}

describe("authentication", () => {
  let client: QueryClient;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/");
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends an unauthenticated visitor to the login route", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(problem(401)));

    renderApp(client);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
    });
    expect(window.location.pathname).toBe("/login");
  });

  it("treats a failed session call as anonymous rather than authenticated", async () => {
    fetchMock.mockImplementation(() => Promise.reject(new Error("network down")));

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <Probe />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId("status")).toHaveTextContent("anonymous");
    });
    expect(screen.getByTestId("editor")).toHaveTextContent("no");
  });

  it("exposes the realm roles Keycloak asserted and answers hasRole from them", async () => {
    fetchMock.mockImplementation(() => Promise.resolve(json(IDENTITY)));

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <AuthProvider>
            <Probe />
          </AuthProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await waitFor(() => {
      expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    });
    expect(screen.getByTestId("roles")).toHaveTextContent("portal-viewer,space-editor");
    expect(screen.getByTestId("editor")).toHaveTextContent("yes");
    expect(screen.getByTestId("admin")).toHaveTextContent("no");
  });

  it("renders the protected shell for a live session", async () => {
    fetchMock.mockImplementation((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (url.includes("/auth/me")) {
        return Promise.resolve(json(IDENTITY));
      }
      return Promise.resolve(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
    });

    renderApp(client);

    await waitFor(() => {
      expect(screen.getByRole("banner")).toBeInTheDocument();
    });
    expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Sign in" })).not.toBeInTheDocument();
  });
});
