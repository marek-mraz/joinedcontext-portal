import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { RATE_LIMIT_CLASSES } from "../src/schemas/kinds";

/**
 * T-0300: representations, rate limit class, cache TTL and the allowed-project list of an
 * Endpoint, edited in the Portal and proposed as a Change (EP-05, EP-14, EP-20, UI-01).
 */

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
        rateLimits: { requestsPerMinute: 600, burst: 60 },
        caching: { maxAgeSeconds: 60 },
      },
      status: { phase: "Live" },
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
    const url = typeof input === "string" ? new URL(input) : new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    // The schema surface of the endpoint the projection panel reads; empty here, because
    // this file is about the configuration blocks and not about the projection.
    if (url.pathname.includes("/api/endpoint/")) {
      return json({ models: [] });
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (request.method !== "GET") {
      return json(CHANGE, 202);
    }
    if (url.pathname.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (url.pathname.endsWith("/endpoints")) {
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
    .filter((request) => request instanceof Request && request.method !== "GET");
}

/** rjsf renders an array editor: one "Add" button, and items that carry an id, not a label. */
async function addAllowedProject(dialog: HTMLElement, name: string) {
  await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
  const input = dialog.querySelector("#root_allowedProjects_0") as HTMLInputElement;
  await userEvent.type(input, name);
}

async function openEditor() {
  const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
  await userEvent.click(within(row).getByRole("button", { name: en.endpoints.edit }));
  return screen.findByRole("dialog");
}

describe("endpoint editor toggles", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("prefills the rate limit class and the cache lifetime the endpoint already carries", async () => {
    renderEndpoints();
    const dialog = await openEditor();

    const rateClass = within(dialog).getByLabelText(
      new RegExp(en.endpoints.field.requestsPerMinute),
    ) as HTMLSelectElement;
    expect(rateClass.selectedOptions[0].textContent).toContain(en.endpoints.rateClass.standard);

    const ttl = within(dialog).getByLabelText(
      new RegExp(en.endpoints.field.maxAge.replace(/[()]/g, "\\$&")),
    ) as HTMLInputElement;
    expect(ttl).toHaveAttribute("type", "range");
    expect(ttl.value).toBe("60");
  });

  it("proposes a new representation, rate limit class and cache lifetime in one Change", async () => {
    const fetchMock = renderEndpoints();
    const dialog = await openEditor();

    await userEvent.click(within(dialog).getByRole("checkbox", { name: "csv" }));
    // rjsf numbers the branches of a `oneOf`, so the option is picked by its label.
    await userEvent.selectOptions(
      within(dialog).getByLabelText(new RegExp(en.endpoints.field.requestsPerMinute)),
      within(dialog).getByRole("option", {
        name: new RegExp(en.endpoints.rateClass.strict),
      }),
    );
    // A range input takes a value change rather than typing into it.
    fireEvent.change(
      within(dialog).getByLabelText(new RegExp(en.endpoints.field.maxAge.replace(/[()]/g, "\\$&"))),
      { target: { value: "120" } },
    );

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as {
      spec: {
        enabledRepresentations: string[];
        rateLimits: { requestsPerMinute: number; burst?: number };
        caching: { maxAgeSeconds: number };
      };
    };
    expect(body.spec.enabledRepresentations).toEqual(["ngsi-ld", "geojson", "csv"]);
    expect(body.spec.rateLimits.requestsPerMinute).toBe(RATE_LIMIT_CLASSES.strict);
    expect(body.spec.rateLimits.burst).toBe(60);
    expect(body.spec.caching.maxAgeSeconds).toBe(120);
  });

  it("never sends allowedProjects on a public endpoint (EP-14, EP-15)", async () => {
    const fetchMock = renderEndpoints();
    const dialog = await openEditor();

    // The audience is `public`, so the manifest refuses an allowed-project list: the form
    // does not even offer the field, and nothing reaches the API rather than a 400 from it.
    expect(within(dialog).queryByText(en.endpoints.field.allowedProjects)).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as {
      spec: Record<string, unknown>;
    };
    expect(body.spec.allowedProjects).toBeUndefined();
  });

  it("says that a public endpoint answers anonymous callers read-only", async () => {
    renderEndpoints();
    const dialog = await openEditor();

    expect(within(dialog).getByText(en.endpoints.publicNotice)).toBeInTheDocument();
  });

  it("omits every optional block a steward left alone on a new endpoint", async () => {
    const fetchMock = renderEndpoints();

    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "mestska-doprava");
    await addAllowedProject(dialog, "doprava");
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as { spec: Record<string, unknown> };
    // An untouched fieldset arrives from the form as `{}`, which `EndpointSpec::validate`
    // refuses, so only a filled-in block reaches the manifest. The rate limit is the one
    // exception: EP-20 wants every endpoint to carry one, so the form starts at Standard.
    expect(body.spec.rateLimits).toEqual({ requestsPerMinute: RATE_LIMIT_CLASSES.standard });
    expect(body.spec.caching).toBeUndefined();
    expect(body.spec.projection).toBeUndefined();
    // A new endpoint defaults to `project-list`, where the list is required.
    expect(body.spec.allowedProjects).toEqual(["doprava"]);
  });
});
