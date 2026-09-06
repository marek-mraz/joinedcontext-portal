import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { generateSlug, SLUG_PATTERN } from "../src/schemas/kinds";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: {
        name: "public-air",
        namespace: "banskabystrica",
        labels: { "joinedcontext.com/space": "ovzdusie" },
      },
      spec: {
        contextSpaceRef: "ovzdusie",
        slug: SLUG,
        audience: "public",
        enabledRepresentations: ["ngsi-ld", "geojson"],
      },
      status: {
        phase: "Live",
        sourceUrl:
          "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/spaces/ovzdusie/endpoints/public-air.yaml",
      },
    },
  ],
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "ovzdusie", namespace: "banskabystrica" },
      spec: {},
      status: { phase: "Live" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

function renderEndpoints() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
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
    if (request.method !== "GET") {
      return json(CHANGE, 202);
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
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

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method !== "GET");
}

describe("endpoints view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("generates a base32 slug with at least 128 bits of entropy (EP-02)", () => {
    const slugs = new Set<string>();
    for (let i = 0; i < 50; i += 1) {
      const slug = generateSlug();
      expect(slug).toMatch(new RegExp(SLUG_PATTERN));
      expect(slug).toHaveLength(26);
      slugs.add(slug);
    }
    // 130 bits: 50 draws colliding would mean the generator is not random at all.
    expect(slugs.size).toBe(50);
  });

  it("shows the audience and every enabled representation of an endpoint", async () => {
    renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    expect(within(row).getByText(en.endpoints.audience.public)).toBeInTheDocument();
    expect(within(row).getByText("ngsi-ld")).toBeInTheDocument();
    expect(within(row).getByText("geojson")).toBeInTheDocument();
    expect(within(row).queryByText("csv")).not.toBeInTheDocument();
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();
  });

  it("copies the public URL of an endpoint", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.endpoints.copyUrl }));

    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/api/endpoint/${SLUG}`);
    expect(await within(row).findByRole("button", { name: en.endpoints.copied })).toBeInTheDocument();
  });

  it("prefills a fresh slug for a new endpoint and regenerates it on demand", async () => {
    renderEndpoints();

    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");
    const slugField = within(dialog).getByLabelText(/Slug/) as HTMLInputElement;
    const first = slugField.value;
    expect(first).toMatch(new RegExp(SLUG_PATTERN));

    await userEvent.type(within(dialog).getByLabelText(/^Name/), "mestska-doprava");
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.generateSlug }));

    await waitFor(() => expect(slugField.value).not.toBe(first));
    // Regenerating the slug must not throw away what the user already typed.
    expect((within(dialog).getByLabelText(/^Name/) as HTMLInputElement).value).toBe(
      "mestska-doprava",
    );
  });

  it("proposes an added representation on an existing endpoint (DEMO step 7)", async () => {
    const fetchMock = renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.endpoints.edit }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("checkbox", { name: "ngsi-ld" })).toBeChecked();
    const csv = within(dialog).getByRole("checkbox", { name: "csv" });
    expect(csv).not.toBeChecked();
    await userEvent.click(csv);

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/endpoints/public-air",
    );
    const body = (await request.clone().json()) as {
      metadata: { labels: Record<string, string> };
      spec: { enabledRepresentations: string[] };
    };
    expect(body.spec.enabledRepresentations).toEqual(["ngsi-ld", "geojson", "csv"]);
    // The Endpoint path template carries a {space}, which the API reads from this label.
    expect(body.metadata.labels["joinedcontext.com/space"]).toBe("ovzdusie");

    expect(await screen.findByText("chg-77aa11bb")).toBeInTheDocument();
  });
});
