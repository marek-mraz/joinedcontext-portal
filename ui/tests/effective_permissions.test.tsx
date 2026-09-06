import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { readsOf, writesOf } from "../src/pages/access/EffectivePermissions";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica" },
      spec: { slug: "mluyob4nz52lok3ssk7pgn5vwt", audience: "public" },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "parking-internal", namespace: "banskabystrica" },
      spec: { slug: "zt4qm7ge2xdv6ksb3ncf5arw2y", audience: "organization" },
    },
  ],
};

/** The document the Context Gateway serves on `…/access` (EP-55, EP-56). */
const GRANTS = {
  subject: { type: "user", id: "did:web:banskabystrica.sk:users:janko" },
  resource: { type: "endpoint", id: "mluyob4nz52lok3ssk7pgn5vwt", space: "ovzdusie" },
  permissions: [
    {
      resource: { type: "AirQualityObserved" },
      actions: ["retrieveEntity", "queryEntity", "updateAttrs"],
      attributes: ["pm10", "pm25", "location"],
      constraints: {
        geoQ: "georel=within;geometry=Polygon;coordinates=[[1,2]]",
        temporalQ: "timerel=after;timeAt=P-1D",
      },
    },
    {
      resource: { type: "District", idPatterns: ["urn:ngsi-ld:District:bb:.*"] },
      actions: ["retrieveEntity"],
      attributes: "*",
      constraints: {},
    },
  ],
  prohibitions: [
    {
      resource: { type: "PersonRecord" },
      actions: ["retrieveEntity"],
      attributes: "*",
      constraints: {},
    },
  ],
};

function renderMatrix(document: unknown = GRANTS, accessStatus = 200) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    // The generated client hands over a `Request`; the gateway call is a plain `fetch(url)`.
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/access")) {
      return json(document, accessStatus);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

/** The path of one recorded call, whichever shape the caller used. */
const pathOf = (input: unknown): string => {
  const href =
    typeof input === "string" || input instanceof URL ? String(input) : (input as Request).url;
  return new URL(href, window.location.origin).pathname;
};

const rowOf = async (type: string) =>
  (await screen.findByRole("rowheader", { name: new RegExp(type) })).closest(
    "tr",
  ) as HTMLElement;

describe("effective permissions", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/access");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("splits CIM 009 operations into reads and writes", () => {
    expect(readsOf(["retrieveEntity", "queryEntity", "updateAttrs"])).toEqual([
      "retrieveEntity",
      "queryEntity",
    ]);
    expect(writesOf(["retrieveEntity", "updateAttrs", "deleteEntity"])).toEqual([
      "updateAttrs",
      "deleteEntity",
    ]);
    expect(writesOf(["queryTemporal"])).toEqual([]);
  });

  it("renders one row per entity type with its operations, attributes and residual", async () => {
    renderMatrix();

    const air = await rowOf("AirQualityObserved");
    expect(within(air).getByText("retrieveEntity")).toBeInTheDocument();
    expect(within(air).getByText("queryEntity")).toBeInTheDocument();
    expect(within(air).getByText("updateAttrs")).toBeInTheDocument();
    expect(within(air).getByText("pm10")).toBeInTheDocument();
    expect(within(air).getByText(/georel=within/)).toBeInTheDocument();
    expect(within(air).getByText(/timerel=after/)).toBeInTheDocument();

    const district = await rowOf("District");
    expect(within(district).getByText(en.access.matrix.allAttributes)).toBeInTheDocument();
    expect(within(district).getByText(en.access.matrix.unconstrained)).toBeInTheDocument();
    expect(within(district).getByText(/urn:ngsi-ld:District:bb/)).toBeInTheDocument();
    // Read-only: nothing in the write column of a type the caller may only read.
    expect(within(district).getAllByText(en.access.matrix.none).length).toBeGreaterThan(0);
  });

  it("reads the endpoint's own access surface, never a second implementation (EP-60)", async () => {
    const fetchMock = renderMatrix();
    await screen.findByRole("rowheader", { name: /AirQualityObserved/ });

    const asked = fetchMock.mock.calls
      .map((call) => pathOf(call[0]))
      .find((path) => path.endsWith("/access"));
    expect(asked).toBe("/api/endpoint/mluyob4nz52lok3ssk7pgn5vwt/access");
  });

  it("asks the endpoint the reader picks", async () => {
    const fetchMock = renderMatrix();
    await screen.findByRole("rowheader", { name: /AirQualityObserved/ });

    await userEvent.selectOptions(
      screen.getByLabelText(en.access.matrix.endpoint),
      "zt4qm7ge2xdv6ksb3ncf5arw2y",
    );

    await vi.waitFor(() => {
      const paths = fetchMock.mock.calls.map((call) => pathOf(call[0]));
      expect(paths).toContain("/api/endpoint/zt4qm7ge2xdv6ksb3ncf5arw2y/access");
    });
  });

  it("shows denials the document names and nothing about types it does not (EP-59)", async () => {
    renderMatrix();

    expect(
      await screen.findByRole("heading", { name: en.access.matrix.prohibitions }),
    ).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: /PersonRecord/ })).toBeInTheDocument();
    // A type no policy names is simply absent: the view must not invent a "denied" row for it.
    expect(screen.queryByText(/ParkingSpot/)).not.toBeInTheDocument();
  });

  it("says nothing is readable rather than showing an empty table", async () => {
    renderMatrix({ subject: { id: "anonymous" }, permissions: [], prohibitions: [] });

    expect(await screen.findByText(en.access.matrix.empty)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("explains a refusal instead of rendering a broken table", async () => {
    renderMatrix({ status: 403 }, 403);

    expect(await screen.findByText(en.access.matrix.forbidden)).toBeInTheDocument();
  });
});
