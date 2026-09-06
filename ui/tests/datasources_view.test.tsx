import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { endpointOf, knownSecretNames, toEnvelope } from "../src/pages/datasources/DataSourcesPage";
import type { Manifest } from "../src/api/manifest";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SOURCES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: {
        name: "mqtt-mesto",
        namespace: "banskabystrica",
        title: { sk: "Mestský MQTT broker", en: "City MQTT broker" },
      },
      spec: {
        type: "mqtt",
        mqtt: {
          urls: ["tls://mqtt.banskabystrica.sk:8883"],
          topics: ["sensors/aq/+/reading"],
          username: "bb-collector",
          passwordRef: { name: "mqtt-mesto", key: "password" },
        },
      },
      status: { phase: "Live" },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "mhd-vehicles", namespace: "banskabystrica" },
      spec: {
        type: "gtfs-rt",
        gtfsRt: { url: "https://gtfs.banskabystrica.sk/vp.pb", feed: "vehiclePositions" },
      },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-7f3e", namespace: "banskabystrica" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};

const DRY_RUN = {
  valid: true,
  lane: "green",
  plan: {
    fields: [{ path: "spec.mqtt.urls", to: ["tls://mqtt.banskabystrica.sk:8883"] }],
  },
};

function renderDataSources() {
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
      return url.searchParams.get("dryRun") === "All" ? json(DRY_RUN) : json(CHANGE, 202);
    }
    if (url.pathname.endsWith("/datasources")) {
      return json(SOURCES);
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

describe("data sources view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/datasources");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every source with its type, endpoint and the names of its credentials", async () => {
    renderDataSources();

    // The name is in two columns, its own and the credentials, so the cells are read in order.
    const row = (await screen.findByText("City MQTT broker")).closest("tr") as HTMLElement;
    const cells = within(row).getAllByRole("cell");
    expect(cells[0]).toHaveTextContent("mqtt-mesto");
    expect(cells[1]).toHaveTextContent(en.datasources.type.mqtt);
    expect(cells[2]).toHaveTextContent("tls://mqtt.banskabystrica.sk:8883");
    expect(cells[3]).toHaveTextContent("mqtt-mesto");

    const feed = (await screen.findByText("mhd-vehicles")).closest("tr") as HTMLElement;
    expect(within(feed).getByText(en.datasources.type["gtfs-rt"])).toBeInTheDocument();
    expect(within(feed).getByText(en.datasources.noSecret)).toBeInTheDocument();
  });

  it("renders the MQTT form for the MQTT type and no other connection", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");

    expect(within(dialog).getByLabelText(/Broker URLs/)).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/User name/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Opening message/)).not.toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Feed/)).not.toBeInTheDocument();
  });

  it("swaps the form to the HTTP connection when the type selector changes", async () => {
    renderDataSources();

    await userEvent.selectOptions(
      await screen.findByLabelText(en.datasources.field.type),
      "http",
    );
    await userEvent.click(screen.getByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");

    expect(within(dialog).getByLabelText(/Method/)).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/Timeout/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/Broker URLs/)).not.toBeInTheDocument();
  });

  /** The value of a credential is never a field; only the reference to it is (CC-06, MF-35). */
  it("asks for the name and the key of a secret, never for the secret", async () => {
    renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");

    expect(within(dialog).getByText(en.datasources.field.password)).toBeInTheDocument();
    expect(within(dialog).getByText(en.datasources.secretHint)).toBeInTheDocument();
    const password = within(dialog).queryByLabelText(/^Password$/);
    expect(password).toBeNull();

    // The picker offers the names the project already uses, and nothing from the store itself.
    const name = within(dialog).getAllByLabelText(/Secret name/)[0];
    const list = name.getAttribute("list");
    expect(list).toBeTruthy();
    const options = document.getElementById(list as string)?.querySelectorAll("option") ?? [];
    expect([...options].map((option) => option.getAttribute("value"))).toContain("mqtt-mesto");
  });

  it("refuses a name that is not a lowercase slug before anything is sent", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "MQTT Mesta");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));

    const alerts = await within(dialog).findAllByRole("alert");
    expect(alerts.some((alert) => alert.textContent?.includes(en.form.pattern))).toBe(true);
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("proposes a change carrying the manifest the form describes", async () => {
    const fetchMock = renderDataSources();

    await userEvent.selectOptions(
      await screen.findByLabelText(en.datasources.field.type),
      "web-socket",
    );
    await userEvent.click(screen.getByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "aq-stream");
    await userEvent.type(
      within(dialog).getByLabelText(/URL/),
      "wss://feed.banskabystrica.sk/aq",
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/datasources");
    await expect(request.clone().json()).resolves.toMatchObject({
      kind: "DataSource",
      metadata: { name: "aq-stream", namespace: "banskabystrica" },
      spec: { type: "web-socket", webSocket: { url: "wss://feed.banskabystrica.sk/aq" } },
    });

    expect(await screen.findByText("chg-7f3e")).toBeInTheDocument();
  });

  /** MF-13: the plan is a dry run against the same route, not a second endpoint. */
  it("shows the planned change before the proposal is sent", async () => {
    const fetchMock = renderDataSources();

    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mqtt-novy");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.check }));

    expect(await within(dialog).findByText("spec.mqtt.urls")).toBeInTheDocument();
    const dry = writes(fetchMock);
    expect(dry).toHaveLength(1);
    expect(new URL(dry[0].url).searchParams.get("dryRun")).toBe("All");
  });

  it("edits an existing source in place, at its own path and its own type", async () => {
    const fetchMock = renderDataSources();

    const row = (await screen.findByText("City MQTT broker")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.datasources.edit }));
    const dialog = await screen.findByRole("dialog");

    expect(within(dialog).getByLabelText(/Name/)).toHaveValue("mqtt-mesto");
    expect(within(dialog).getByLabelText(/User name/)).toHaveValue("bb-collector");
    await userEvent.clear(within(dialog).getByLabelText(/User name/));
    await userEvent.type(within(dialog).getByLabelText(/User name/), "bb-reader");
    await userEvent.click(within(dialog).getByRole("button", { name: en.datasources.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/datasources/mqtt-mesto",
    );
    await expect(request.clone().json()).resolves.toMatchObject({
      spec: { type: "mqtt", mqtt: { username: "bb-reader" } },
    });
  });
});

describe("the manifest a data source form describes", () => {
  it("puts the metadata in metadata and everything else under the declared type", () => {
    const envelope = toEnvelope("banskabystrica", "http", {
      name: "aq-opendata",
      title: { en: "Open data" },
      http: { url: "https://opendata.banskabystrica.sk/aq.json", verb: "GET" },
      // rjsf leaves this behind for a group the user opened and left alone.
      tls: { caCertRef: {} },
    });

    expect(envelope).toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: {
        name: "aq-opendata",
        namespace: "banskabystrica",
        title: { en: "Open data" },
      },
      spec: {
        type: "http",
        http: { url: "https://opendata.banskabystrica.sk/aq.json", verb: "GET" },
      },
    });
  });

  it("reads the endpoint and the credential names off a stored manifest", () => {
    const [mqtt, gtfs] = SOURCES.items;
    expect(endpointOf(mqtt.spec)).toBe("tls://mqtt.banskabystrica.sk:8883");
    expect(endpointOf(gtfs.spec)).toBe("https://gtfs.banskabystrica.sk/vp.pb");
    expect(knownSecretNames(SOURCES.items as unknown as Manifest[])).toEqual(["mqtt-mesto"]);
  });
});
