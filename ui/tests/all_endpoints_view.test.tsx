import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const LIST = "joinedcontext.com/v1alpha1";

function endpoint(project: string, space: string, name: string, slug: string, audience: string) {
  return {
    apiVersion: LIST,
    kind: "Endpoint",
    metadata: { name, namespace: project, labels: { "joinedcontext.com/space": space } },
    spec: { contextSpaceRef: space, slug, audience, enabledRepresentations: ["ngsi-ld", "geojson"] },
    status: { phase: "Live" },
  };
}

/** The dev repository: banskabystrica publishes one endpoint, helsinki two. */
const BY_PATH: Record<string, unknown> = {
  "/api/v1/projects": { apiVersion: LIST, kind: "List", items: [{ name: "banskabystrica" }, { name: "helsinki" }] },
  "/api/v1/projects/banskabystrica/endpoints": {
    apiVersion: LIST,
    kind: "List",
    items: [endpoint("banskabystrica", "ovzdusie", "public-air", "k7m2qz4tv6xh3n5jb2ryd3wcfa", "public")],
  },
  "/api/v1/projects/helsinki/endpoints": {
    apiVersion: LIST,
    kind: "List",
    items: [
      endpoint("helsinki", "helsinki", "helsinki-bikes", "a1b2c3d4e5f6g7h8j9k0m1n2p3", "public"),
      endpoint("helsinki", "helsinki", "helsinki-events", "z9y8x7w6v5u4t3s2r1q0p9n8m7", "organization"),
    ],
  },
};

describe("all endpoints view", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  let responses: Record<string, unknown>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/endpoints");
    responses = { ...BY_PATH };
  });

  function renderAt() {
    fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      const body = url.pathname.includes("/auth/me")
        ? IDENTITY
        : (responses[url.pathname] ?? { apiVersion: LIST, kind: "List", items: [] });
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    });
    vi.stubGlobal("fetch", fetchMock);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every endpoint of every project with its links and a way inside the space", async () => {
    renderAt();
    const table = await screen.findByRole("table", { name: "All endpoints" });
    await waitFor(() => {
      expect(within(table).getAllByRole("row")).toHaveLength(4);
    });

    const bb = within(table).getByText("public-air").closest("tr") as HTMLTableRowElement;
    expect(within(bb).getByRole("link", { name: "banskabystrica" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/endpoints",
    );
    expect(within(bb).getByRole("link", { name: "Look inside: banskabystrica/ovzdusie" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/spaces/ovzdusie",
    );
    expect(within(bb).getByText("Public")).toBeInTheDocument();
    expect(within(bb).getByRole("link", { name: "ngsi-ld" })).toHaveAttribute(
      "href",
      `${window.location.origin}/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ngsi-ld/v1/entities?limit=20`,
    );
    expect(within(bb).getByRole("link", { name: "geojson" })).toHaveAttribute(
      "href",
      `${window.location.origin}/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/file.geojson`,
    );
    expect(within(bb).getByRole("link", { name: "index" })).toHaveAttribute(
      "href",
      `${window.location.origin}/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/`,
    );
    expect(within(bb).getByRole("link", { name: "catalogue" })).toHaveAttribute(
      "href",
      `https://data.${window.location.host}/dataset/public-air`,
    );

    const hel = within(table).getByText("helsinki-events").closest("tr") as HTMLTableRowElement;
    expect(within(hel).getByText("Organization")).toBeInTheDocument();
    expect(within(table).getByText("helsinki-bikes")).toBeInTheDocument();

    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).toContain("/api/v1/projects/banskabystrica/endpoints");
    expect(paths).toContain("/api/v1/projects/helsinki/endpoints");
  });

  it("says so when no project publishes an endpoint", async () => {
    responses["/api/v1/projects/helsinki/endpoints"] = { apiVersion: LIST, kind: "List", items: [] };
    responses["/api/v1/projects/banskabystrica/endpoints"] = { apiVersion: LIST, kind: "List", items: [] };
    renderAt();
    expect(await screen.findByText("No project publishes an endpoint yet.")).toBeInTheDocument();
  });
});
