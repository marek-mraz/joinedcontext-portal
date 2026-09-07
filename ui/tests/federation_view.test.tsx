import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { claimedTypes, targetOf, toEnvelope } from "../src/pages/federation/FederationPage";
import { positionsOf } from "../src/pages/federation/FederationGraph";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

/** The address of the source elsewhere. It must reach no part of the page (UI-27). */
const EXTERNAL_URL = "https://context.zvolen.sk/ngsi-ld/v1";

const REGISTRATIONS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSourceRegistration",
      metadata: {
        name: "mesto-ovzdusie",
        namespace: "banskabystrica",
        title: { sk: "Ovzdušie mesta", en: "City air quality" },
      },
      spec: {
        contextSpaceRef: "hub",
        endpointRef: { kind: "Endpoint", name: "mesto-read" },
        information: [{ entities: [{ type: "AirQualityObserved" }], propertyNames: ["pm10"] }],
        federation: {
          identity: "serviceAccount",
          serviceAccountRef: { kind: "ServiceAccount", name: "hub-reader" },
        },
        mode: "exclusive",
      },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSourceRegistration",
      metadata: { name: "zvolen-ovzdusie", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "hub",
        endpoint: EXTERNAL_URL,
        information: [
          { entities: [{ type: "AirQualityObserved" }, { type: "WeatherObserved" }] },
        ],
        federation: { identity: "caller" },
      },
    },
  ],
};

const GRAPH = {
  nodes: [
    { id: "ContextSpace/hub", kind: "ContextSpace", name: "hub", health: "ok" },
    {
      id: "ContextSourceRegistration/zvolen-ovzdusie",
      kind: "ContextSourceRegistration",
      name: "zvolen-ovzdusie",
      health: "degraded",
      registration: {
        mode: "inclusive",
        identity: "caller",
        types: ["AirQualityObserved", "WeatherObserved"],
        external: true,
      },
    },
    {
      id: "ExternalSource/zvolen-ovzdusie",
      kind: "ExternalSource",
      name: "zvolen-ovzdusie",
      health: "unknown",
    },
  ],
  edges: [
    {
      from: "ContextSourceRegistration/zvolen-ovzdusie",
      to: "ContextSpace/hub",
      kind: "registers",
      manifest: "ContextSourceRegistration/zvolen-ovzdusie",
    },
    {
      from: "ContextSourceRegistration/zvolen-ovzdusie",
      to: "ExternalSource/zvolen-ovzdusie",
      kind: "registers",
      manifest: "ContextSourceRegistration/zvolen-ovzdusie",
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-9a2b", namespace: "banskabystrica" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 1 } },
};

function renderFederation() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (request.method === "POST" || request.method === "PUT") {
      return json(CHANGE, 202);
    }
    if (url.pathname.endsWith("/federation-graph")) {
      return json(GRAPH);
    }
    if (url.pathname.endsWith("/csrs")) {
      return json(REGISTRATIONS);
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

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method === "POST" || request.method === "PUT");
}

describe("federation view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/federation");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every registration with its space, its target and what it claims", async () => {
    renderFederation();

    const row = (await screen.findByText("City air quality")).closest("tr") as HTMLElement;
    const cells = within(row).getAllByRole("cell");
    expect(cells[0]).toHaveTextContent("mesto-ovzdusie");
    expect(cells[1]).toHaveTextContent("hub");
    expect(cells[2]).toHaveTextContent("mesto-read");
    expect(cells[3]).toHaveTextContent("AirQualityObserved");
    expect(cells[4]).toHaveTextContent(en.federation.identity.serviceAccount);

    // The source elsewhere is named after its registration, never after its address. Scoped to
    // the table: the same name is on the graph node, which is the point of the next case.
    const table = screen.getByRole("table");
    const external = within(table)
      .getAllByRole("row")
      .find((candidate) => within(candidate).queryAllByRole("cell")[0]?.textContent === "zvolen-ovzdusie") as HTMLElement;
    expect(within(external).getAllByRole("cell")[2]).toHaveTextContent("zvolen-ovzdusie");
  });

  /** UI-27: selecting a node opens its card, and selecting it again closes it. */
  it("opens the card of the node the reader selects", async () => {
    renderFederation();

    const node = await screen.findByRole("button", {
      name: `${en.federation.kind.ContextSourceRegistration}: zvolen-ovzdusie`,
    });
    await userEvent.click(node);

    const card = await screen.findByRole("complementary", { name: en.federation.card.title });
    expect(within(card).getByText(en.federation.health.degraded)).toBeInTheDocument();
    expect(within(card).getByText(en.federation.identity.caller)).toBeInTheDocument();
    expect(within(card).getByText(en.federation.mode.inclusive)).toBeInTheDocument();
    expect(within(card).getByText(/AirQualityObserved, WeatherObserved/)).toBeInTheDocument();

    await userEvent.click(node);
    expect(
      screen.queryByRole("complementary", { name: en.federation.card.title }),
    ).not.toBeInTheDocument();
  });

  /** UI-27: every edge is drawn and says what it means, so the picture can be read. */
  it("draws one edge per relation the answer carries", async () => {
    renderFederation();

    await screen.findByRole("group", { name: en.federation.graph.title });
    const graph = screen.getByRole("group", { name: en.federation.graph.title });
    const edges = graph.querySelectorAll("path[aria-label]");
    expect(edges).toHaveLength(GRAPH.edges.length);
    for (const edge of edges) {
      expect(edge.getAttribute("aria-label")).toBe(en.federation.edge.registers);
    }
  });

  /** UI-27, PF-48: no address and no account name is anywhere on the page. */
  it("shows that a registration authenticates, never with what", async () => {
    renderFederation();

    await screen.findByRole("group", { name: en.federation.graph.title });
    const node = screen.getByRole("button", {
      name: `${en.federation.kind.ContextSourceRegistration}: zvolen-ovzdusie`,
    });
    await userEvent.click(node);
    await screen.findByRole("complementary", { name: en.federation.card.title });

    const page = document.body.textContent ?? "";
    for (const leaked of [EXTERNAL_URL, "context.zvolen.sk", "hub-reader"]) {
      expect(page).not.toContain(leaked);
    }
    expect(page).toContain(en.federation.card.noCredential);
  });

  it("proposes a registration carrying the manifest the form describes", async () => {
    const fetchMock = renderFederation();

    await userEvent.click(await screen.findByRole("button", { name: en.federation.add }));
    const dialog = await screen.findByRole("dialog");
    // By id, not by label: a registration has a name and so do the endpoint and the account it
    // may reference, and all three are correctly labelled "Name" under their own group.
    const field = (id: string) => dialog.querySelector(`#${id}`) as HTMLElement;
    await userEvent.type(field("root_name"), "zvolen-doprava");
    await userEvent.type(field("root_contextSpaceRef"), "hub");
    // A registration that claims nothing is matched by nothing, so the form opens with one
    // empty claim and will not submit until it says which type the source holds.
    await userEvent.type(field("root_information_0_entities_0_type"), "TrafficFlowObserved");
    await userEvent.click(
      within(dialog).getByRole("button", { name: en.federation.propose }),
    );

    await waitFor(() => expect(writes(fetchMock).length).toBeGreaterThan(0));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/csrs");
    await expect(request.clone().json()).resolves.toMatchObject({
      kind: "ContextSourceRegistration",
      metadata: { name: "zvolen-doprava", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "hub",
        information: [{ entities: [{ type: "TrafficFlowObserved" }] }],
      },
    });

    expect(await screen.findByText("chg-9a2b")).toBeInTheDocument();
  });
});

describe("what a registration form describes", () => {
  it("puts the metadata in metadata and everything else in the spec", () => {
    expect(
      toEnvelope("banskabystrica", {
        name: "zvolen-ovzdusie",
        title: { en: "Zvolen" },
        contextSpaceRef: "hub",
        endpoint: EXTERNAL_URL,
        information: [{ entities: [{ type: "AirQualityObserved" }] }],
        // rjsf leaves this behind for a group the author opened and left alone.
        federation: { serviceAccountRef: {} },
      }),
    ).toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSourceRegistration",
      metadata: {
        name: "zvolen-ovzdusie",
        namespace: "banskabystrica",
        title: { en: "Zvolen" },
      },
      spec: {
        contextSpaceRef: "hub",
        endpoint: EXTERNAL_URL,
        information: [{ entities: [{ type: "AirQualityObserved" }] }],
      },
    });
  });

  it("reads the target and the claimed types off a stored manifest", () => {
    const [local, external] = REGISTRATIONS.items;
    expect(targetOf(local.spec, local.metadata.name)).toBe("mesto-read");
    expect(targetOf(external.spec, external.metadata.name)).toBe("zvolen-ovzdusie");
    expect(claimedTypes(external.spec)).toEqual(["AirQualityObserved", "WeatherObserved"]);
  });

  /** The same input draws the same picture, so a selection survives a refetch. */
  it("places the spaces in the middle and everything else around them", () => {
    const positions = positionsOf(GRAPH.nodes as never);
    expect(positions["ContextSpace/hub"]).toEqual({ x: 50, y: 50 });
    expect(positions["ExternalSource/zvolen-ovzdusie"]).not.toEqual({ x: 50, y: 50 });
    expect(positionsOf(GRAPH.nodes as never)).toEqual(positions);
  });
});
