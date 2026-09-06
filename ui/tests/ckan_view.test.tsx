import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-approver"],
};

const SLUG = "zt4qm7ge2xdv6ksb3ncf5arw2y";

const STATUS = {
  instances: [
    {
      name: "open-data",
      url: "https://data.banskabystrica.sk",
      organizationDefault: "mesto-banska-bystrica",
      apiTokenRef: "ckan-open-data",
    },
  ],
  publications: [
    {
      endpoint: "ovzdusie-public",
      phase: "Live",
      instance: "open-data",
      instanceMissing: false,
      organization: "mesto-banska-bystrica",
      dataset: "kvalita-ovzdusia",
      datasetUrl: "https://data.banskabystrica.sk/dataset/kvalita-ovzdusia",
      resources: [
        {
          name: "NGSI-LD API",
          url: `https://bb.example.com/api/endpoint/${SLUG}/ngsi-ld/v1/`,
          format: "NGSI-LD",
        },
        {
          name: "CSV",
          url: `https://bb.example.com/api/endpoint/${SLUG}/file.csv`,
          format: "CSV",
        },
      ],
      datastore: { representation: "csv", refresh: "onChange" },
    },
    {
      endpoint: "hluk-public",
      instance: "gone",
      instanceMissing: true,
      dataset: "hluk",
      resources: [],
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

function renderCkan(status: unknown = STATUS) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, code = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status: code,
          headers: {
            "Content-Type": code >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json({ instanceName: "joinedcontext", languages: { default: "en", offered: ["en"] } });
    }
    if (path.endsWith("/ckan/status")) {
      return json(status);
    }
    if (request.method !== "GET") {
      return json(CHANGE, 202);
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

describe("ckan publishing manager", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/ckan");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the catalogue by its token reference and never a token (EP-67)", async () => {
    renderCkan();

    const row = (await screen.findByText("open-data")).closest("tr") as HTMLElement;
    expect(within(row).getByText("https://data.banskabystrica.sk")).toBeInTheDocument();
    expect(within(row).getByText("mesto-banska-bystrica")).toBeInTheDocument();
    expect(within(row).getByText("ckan-open-data")).toBeInTheDocument();
    // The form asks for the name of a secret, not for a credential to paste.
    expect(screen.getByText(en.ckan.instances.tokenHelp)).toBeInTheDocument();
    expect(document.querySelector("input[type='password']")).toBeNull();
  });

  it("shows the dataset link and one link per published resource (EP-64)", async () => {
    renderCkan();

    const dataset = await screen.findByRole("link", { name: "kvalita-ovzdusia" });
    expect(dataset).toHaveAttribute(
      "href",
      "https://data.banskabystrica.sk/dataset/kvalita-ovzdusia",
    );
    const entry = dataset.closest("li") as HTMLElement;
    expect(within(entry).getByRole("link", { name: "NGSI-LD" })).toHaveAttribute(
      "href",
      `https://bb.example.com/api/endpoint/${SLUG}/ngsi-ld/v1/`,
    );
    expect(within(entry).getByRole("link", { name: "CSV" })).toHaveAttribute(
      "href",
      `https://bb.example.com/api/endpoint/${SLUG}/file.csv`,
    );
    expect(within(entry).getByText(/DataStore mirror from csv/)).toBeInTheDocument();
    expect(within(entry).getByText("published to open-data")).toBeInTheDocument();
  });

  it("says which endpoint points at a catalogue that is not there", async () => {
    renderCkan();

    const chip = await screen.findByText("catalogue gone is missing");
    const entry = chip.closest("li") as HTMLElement;
    expect(within(entry).getByText("hluk-public")).toBeInTheDocument();
    expect(within(entry).queryByRole("link")).toBeNull();
  });

  it("proposes a catalogue as a change and never sends a credential (CC-03, MF-24)", async () => {
    const fetchMock = renderCkan();
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText(en.ckan.instances.name), "open-data-2");
    const url = screen.getByLabelText(en.ckan.instances.url);
    await user.clear(url);
    await user.type(url, "https://data.example.org");
    await user.type(screen.getByLabelText(en.ckan.instances.organization), "mesto");
    await user.type(screen.getByLabelText(en.ckan.instances.tokenRef), "ckan-second");
    await user.click(screen.getByRole("button", { name: en.ckan.instances.propose }));

    const write = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.method === "POST");
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(new URL(write.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/ckaninstances",
    );
    const body = JSON.parse(await write.clone().text()) as Record<string, never>;
    expect(body.kind).toBe("CkanInstance");
    expect(body.spec).toMatchObject({
      url: "https://data.example.org",
      organizationDefault: "mesto",
      apiTokenRef: { name: "ckan-second", key: "apiToken" },
    });
    const sent = JSON.stringify(body);
    for (const forbidden of ["apiToken\":\"", "password", "\"token\""]) {
      expect(sent).not.toContain(forbidden);
    }
    // The steward is told where the proposal went, as with every other write.
    expect(await screen.findByText(/chg-0000a1b2/)).toBeInTheDocument();
  });

  it("says so when the project publishes nowhere", async () => {
    renderCkan({ instances: [], publications: [] });

    expect(await screen.findByText(en.ckan.instances.empty)).toBeInTheDocument();
    expect(screen.getByText(en.ckan.publications.empty)).toBeInTheDocument();
  });
});
