/** T-0517: filters generated from the DataModel, shared by the explorer and the pipeline studio (UI-33, DM-20, PL-42). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import type { Manifest } from "../src/api/manifest";
import { filterSlotsOf, filtersFromQ, opsFor, qFromFilters } from "../src/components/entities/filters";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const BRANDING = {
  name: "Portal",
  colors: { primary: "#1d4ed8", background: "#ffffff", text: "#0f172a" },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const AIR_MODEL = [
  "id: https://banskabystrica.sk/models/air",
  "name: bb-air-quality",
  "classes:",
  "  AirQualityObserved:",
  "    slots: [id, pm10, active, level, refDistrict, dateObserved]",
  "slots:",
  "  id: {}",
  "  pm10: { range: float, minimum_value: 0 }",
  "  active: { range: boolean }",
  "  level: { range: AirLevel }",
  "  refDistrict:",
  "    range: uri",
  "    annotations: { ngsi_ld_kind: Relationship }",
  "  dateObserved: { range: datetime }",
  "enums:",
  "  AirLevel:",
  "    permissible_values: { good: {}, bad: {} }",
  "",
].join("\n");

const MODEL: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bb-air-quality", namespace: "banskabystrica" },
  spec: { linkml: AIR_MODEL, version: "1.0.0" },
};
const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "ovzdusie", namespace: "banskabystrica", title: { en: "Air quality" } },
    spec: { dataModelRef: "bb-air-quality" },
  },
]);
const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "banskabystrica" },
    spec: { contextSpaceRef: "ovzdusie", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);
const ROWS = [
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:radvan-01", type: "AirQualityObserved", pm10: 12 },
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:sasova-01", type: "AirQualityObserved", pm10: 30 },
];
const FULL = {
  id: ROWS[1].id,
  type: "AirQualityObserved",
  pm10: { type: "Property", value: 30, observedAt: "2026-09-07T10:00:00Z" },
};

describe("filters from the model", () => {
  const slots = filterSlotsOf(MODEL, "AirQualityObserved");

  it("types every slot of the class by its range, enum and kind", () => {
    expect(slots.map((s) => s.name)).toEqual(["pm10", "active", "level", "refDistrict", "dateObserved"]);
    expect(slots[0]).toMatchObject({ range: "float", minimum: 0 });
    expect(slots[2].values).toEqual(["good", "bad"]);
    expect(slots[3].kind).toBe("Relationship");
    expect(filterSlotsOf(MODEL, "Nope")).toEqual([]);
    expect(filterSlotsOf(undefined, "AirQualityObserved")).toEqual([]);
  });

  it("offers the operators the range allows", () => {
    expect(opsFor(slots[0])).toEqual(["==", "!=", ">", ">=", "<", "<="]);
    expect(opsFor(slots[1])).toEqual(["==", "!="]);
    expect(opsFor(slots[2])).toEqual(["==", "!="]);
    expect(opsFor(slots[3])).toEqual(["==", "!="]);
    expect(opsFor(slots[4])).toEqual(["==", "!=", ">", ">=", "<", "<="]);
    expect(opsFor({ name: "name", kind: "Property", range: "string" })).toEqual(["==", "!=", "~=", "!~="]);
    expect(opsFor(undefined)).toHaveLength(8);
  });

  it("composes q with bare numbers, booleans and dates and quoted strings, and parses it back", () => {
    const rows = [
      { attribute: "pm10", op: ">" as const, value: "10" },
      { attribute: "active", op: "==" as const, value: "true" },
      { attribute: "level", op: "!=" as const, value: "bad" },
      { attribute: "refDistrict", op: "==" as const, value: "urn:ngsi-ld:District:bb:d:1" },
      { attribute: "dateObserved", op: ">=" as const, value: "2026-09-01T00:00:00Z" },
      { attribute: "pm10", op: "<" as const, value: "" },
    ];
    const q = qFromFilters(rows, slots);
    expect(q).toBe(
      'pm10>10;active==true;level!="bad";refDistrict=="urn:ngsi-ld:District:bb:d:1";dateObserved>=2026-09-01T00:00:00Z',
    );
    expect(filtersFromQ(q)).toEqual(rows.slice(0, 5));
    // A quote inside a string survives the round trip.
    expect(filtersFromQ(qFromFilters([{ attribute: "name", op: "~=", value: 'a"b' }]))).toEqual([
      { attribute: "name", op: "~=", value: 'a"b' },
    ]);
    // Without a slot the value's shape decides: numbers bare, words quoted.
    expect(qFromFilters([{ attribute: "x", op: "==", value: "3" }, { attribute: "y", op: "==", value: "z" }])).toBe(
      'x==3;y=="z"',
    );
  });

  it("leaves q the rows cannot show as text", () => {
    expect(filtersFromQ("")).toEqual([]);
    expect(filtersFromQ(undefined)).toEqual([]);
    expect(filtersFromQ("pm10>10|pm25>5")).toBeUndefined();
    expect(filtersFromQ("(pm10>10)")).toBeUndefined();
    expect(filtersFromQ("pm10")).toBeUndefined();
    expect(filtersFromQ("pm10.value>10")).toEqual([{ attribute: "pm10.value", op: ">", value: "10" }]);
  });
});

function renderExplore() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, headers: Record<string, string> = {}) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json", ...headers },
        }),
      );
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/branding")) return json(BRANDING);
    if (url.pathname.includes("/ngsi-ld/v1/entities/")) return json(FULL);
    if (url.pathname.includes("/ngsi-ld/v1/entities")) {
      const q = url.searchParams.get("q");
      return json(q ? ROWS.filter((row) => row.pm10 > 20) : ROWS, { "NGSILD-Results-Count": q ? "1" : "2" });
    }
    if (url.pathname.endsWith("/spaces")) return json(SPACES);
    if (url.pathname.endsWith("/endpoints")) return json(ENDPOINTS);
    if (url.pathname.endsWith("/datamodels")) return json(list([MODEL]));
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

const gatewayUrls = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls
    .map((call) => new URL((call[0] as Request).url))
    .filter((url) => url.pathname.includes("/ngsi-ld/v1/entities"));

describe("data explorer (UI-33)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/explore");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("reads a page through the space's endpoint with a filter generated from the model, then one entity in full", async () => {
    const fetchMock = renderExplore();
    await screen.findByRole("heading", { name: en.explore.title });
    await userEvent.selectOptions(await screen.findByLabelText(en.explore.space), "ovzdusie");
    expect(screen.getByLabelText(en.explore.endpoint)).toHaveValue("public-air");
    await userEvent.selectOptions(screen.getByLabelText(en.entities.type), "AirQualityObserved");

    // The whole type first: two rows and the broker's count.
    expect(await screen.findByText(ROWS[1].id)).toBeInTheDocument();
    expect(screen.getByText("2 entities")).toBeInTheDocument();

    // A row from the model: pm10 is a float, so the value is a number input and > is offered.
    await userEvent.click(screen.getByRole("button", { name: en.entities.addFilter }));
    const value = screen.getByLabelText(en.entities.value);
    expect(value).toHaveAttribute("type", "number");
    await userEvent.selectOptions(screen.getByLabelText(en.entities.operator), ">");
    await userEvent.type(value, "20");
    expect(screen.getByLabelText(en.entities.q)).toHaveValue("pm10>20");
    await waitFor(() => expect(screen.queryByText(ROWS[0].id)).not.toBeInTheDocument());
    const last = gatewayUrls(fetchMock).at(-1)!;
    expect(last.pathname).toContain("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/");
    expect(last.searchParams.get("q")).toBe("pm10>20");
    expect(last.searchParams.get("count")).toBe("true");
    expect(screen.getByText("1 entities")).toBeInTheDocument();

    // A q typed by hand outside the simple grammar shows as text, no rows.
    await userEvent.clear(screen.getByLabelText(en.entities.q));
    await userEvent.type(screen.getByLabelText(en.entities.q), "pm10>10|pm10<1");
    expect(screen.getByText(en.entities.advanced)).toBeInTheDocument();

    // One entity in full, read by id.
    await userEvent.click(await screen.findByRole("button", { name: ROWS[1].id }));
    const detail = await screen.findByTestId("explore-entity");
    await within(detail).findByText(/observedAt/);
    expect(gatewayUrls(fetchMock).at(-1)!.pathname).toContain(encodeURIComponent(ROWS[1].id));
  });
});
