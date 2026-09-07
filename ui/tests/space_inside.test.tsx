import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import {
  entityTypesOf,
  parseResultsCount,
  pickReadEndpoint,
  spaceOf,
} from "../src/pages/spaces/SpaceInside";
import type { Manifest } from "../src/api/manifest";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

function manifest(kind: string, name: string, spec: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind,
    metadata: { name, namespace: "banskabystrica" },
    spec,
  } as Manifest;
}

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: {
    name: "ovzdusie",
    namespace: "banskabystrica",
    title: { sk: "Ovzdušie", en: "Air quality" },
  },
  spec: { dataModelRef: "bb-air-quality" },
  status: { phase: "Live" },
};

const MODELS = list([
  manifest("DataModel", "bb-air-quality", {
    linkml: "./bb-air-quality.linkml.yaml",
    version: "2.1.0",
    classes: ["AirQualityObserved", "AirQualityStation"],
  }),
]);

const ENDPOINTS = list([
  {
    ...manifest("Endpoint", "public-air", {
      contextSpaceRef: "ovzdusie",
      slug: SLUG,
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "geojson"],
      policyRef: "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie:public-air-quality",
    }),
    status: { phase: "Live" },
  },
  manifest("Endpoint", "other-space", {
    contextSpaceRef: "doprava",
    slug: "zzzzzzzzzzzzzzzzzzzzzzzzzz",
    audience: "public",
    enabledRepresentations: ["ngsi-ld"],
  }),
]);

const POLICIES = list([
  manifest("Policy", "public-air-quality", {
    contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
    assignee: { kind: "role", id: "public" },
    operations: ["queryEntity", "retrieveEntity"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  }),
]);

const SAMPLES = [
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1", type: "AirQualityObserved", pm10: 12 },
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:2", type: "AirQualityObserved", pm10: 9 },
];

function renderInside(gateway: { status: number; count?: number }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const path = url.pathname;
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json", ...headers },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.startsWith(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`)) {
      if (gateway.status !== 200) {
        return json({ title: "Forbidden" }, gateway.status);
      }
      if (url.searchParams.get("count") === "true") {
        return json([SAMPLES[0]], 200, { "NGSILD-Results-Count": String(gateway.count ?? 0) });
      }
      return json(SAMPLES);
    }
    if (path.endsWith("/spaces/ovzdusie")) {
      return json(SPACE);
    }
    if (path.endsWith("/datamodels")) {
      return json(MODELS);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/policies")) {
      return json(POLICIES);
    }
    return json(list([]));
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

describe("count header parsing", () => {
  it("reads NGSILD-Results-Count as a number, case-insensitively", () => {
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "42" }))).toBe(42);
    expect(parseResultsCount(new Headers({ "ngsild-results-count": " 7 " }))).toBe(7);
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "0" }))).toBe(0);
  });

  it("answers undefined for a missing, empty or non-numeric header", () => {
    expect(parseResultsCount(new Headers())).toBeUndefined();
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "" }))).toBeUndefined();
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "many" }))).toBeUndefined();
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "-1" }))).toBeUndefined();
  });
});

describe("space helpers", () => {
  it("prefers the endpoint without a policy, then the first public one", () => {
    const narrowed = manifest("Endpoint", "a", { audience: "public", policyRef: "urn:x" });
    const open = manifest("Endpoint", "b", { audience: "organization" });
    const secondPublic = manifest("Endpoint", "c", { audience: "public", policyRef: "urn:y" });
    expect(pickReadEndpoint([narrowed, open])?.metadata.name).toBe("b");
    expect(pickReadEndpoint([narrowed, secondPublic])?.metadata.name).toBe("a");
    expect(pickReadEndpoint([manifest("Endpoint", "d", { audience: "organization", policyRef: "urn:z" })])).toBeUndefined();
    expect(pickReadEndpoint([])).toBeUndefined();
  });

  it("names the space from a string ref, an object ref or the space label", () => {
    expect(spaceOf(manifest("Policy", "p", { contextSpaceRef: "ovzdusie" }))).toBe("ovzdusie");
    expect(
      spaceOf(manifest("Policy", "p", { contextSpaceRef: { kind: "ContextSpace", name: "doprava" } })),
    ).toBe("doprava");
    const labelled = manifest("Endpoint", "e", {});
    labelled.metadata.labels = { "joinedcontext.com/space": "voda" };
    expect(spaceOf(labelled)).toBe("voda");
    expect(spaceOf(manifest("Policy", "p", {}))).toBeUndefined();
  });

  it("reads entity types from spec.classes or from an inline LinkML source", () => {
    expect(entityTypesOf(manifest("DataModel", "m", { classes: ["A", "B"] }))).toEqual(["A", "B"]);
    expect(
      entityTypesOf(
        manifest("DataModel", "m", {
          linkml: "id: https://x/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: []\n",
        }),
      ),
    ).toEqual(["AirQualityObserved"]);
    expect(entityTypesOf(manifest("DataModel", "m", { linkml: "./air.linkml.yaml" }))).toEqual([]);
  });
});

describe("space inside view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/ovzdusie");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows each entity type with its live count and sample entities read through the gateway", async () => {
    const fetchMock = renderInside({ status: 200, count: 42 });

    expect(await screen.findByRole("heading", { name: "Air quality" })).toBeInTheDocument();
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    expect(await within(row).findByText("42")).toBeInTheDocument();
    expect(within(row).getByText(SAMPLES[0].id)).toBeInTheDocument();
    expect(within(row).getByText(SAMPLES[1].id)).toBeInTheDocument();

    const gatewayCalls = fetchMock.mock.calls
      .map((call) => new URL((call[0] as Request).url))
      .filter((url) => url.pathname.startsWith("/api/endpoint/"));
    expect(gatewayCalls.some((url) => url.searchParams.get("count") === "true" && url.searchParams.get("limit") === "1")).toBe(true);
    expect(gatewayCalls.some((url) => url.searchParams.get("options") === "keyValues" && url.searchParams.get("limit") === "3")).toBe(true);
    expect(gatewayCalls.every((url) => url.origin === window.location.origin)).toBe(true);
  });

  it("lists only the endpoints and policies of this space, with a catalogue link per endpoint", async () => {
    renderInside({ status: 200, count: 1 });

    const endpointRow = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    expect(within(endpointRow).getByText(en.endpoints.audience.public)).toBeInTheDocument();
    expect(within(endpointRow).getByRole("link", { name: "ngsi-ld" })).toHaveAttribute(
      "href",
      `${window.location.origin}/api/endpoint/${SLUG}/ngsi-ld/v1/entities?limit=20`,
    );
    expect(
      within(endpointRow).getByRole("link", { name: en.spaces.inside.catalogueLink }),
    ).toHaveAttribute("href", `https://data.${window.location.host}/dataset/public-air`);
    expect(screen.queryByText("other-space")).not.toBeInTheDocument();

    const policyRow = (await screen.findByText("public-air-quality")).closest("tr") as HTMLElement;
    expect(within(policyRow).getByText("role:public")).toBeInTheDocument();
    expect(within(policyRow).getByText("queryEntity, retrieveEntity")).toBeInTheDocument();
  });

  it("says a type is not readable anonymously when the gateway refuses", async () => {
    renderInside({ status: 403 });

    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    expect(await within(row).findByText(en.spaces.inside.notReadable)).toBeInTheDocument();
  });
});
