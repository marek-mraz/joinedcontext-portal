/**
 * "Generate your own app" (T-0312, AP-22, AP-30, AG-26).
 *
 * The two things worth asserting are the ones a user cannot check for themselves: that the
 * app's grant is derived from the endpoint it is bound to and can only be narrowed, and that
 * a deployment without a builder agent says so rather than opening a merge request nothing
 * will pick up.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { BLUEPRINT, dataNeeds } from "../src/pages/apps/AppGenerator";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["domain-editor"],
};

const PROJECT = "banskabystrica";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const BLUEPRINT_CARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Blueprint",
  metadata: { name: BLUEPRINT, namespace: "org", title: { en: "App from a prompt" } },
  spec: { version: "1.4.0", riskClass: "yellow" },
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "ovzdusie-public", namespace: PROJECT, title: { en: "Air quality open data" } },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: SLUG,
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "geojson"],
  },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000c3d4", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 3 } },
};

/** The projected draft-07 document the endpoint serves, which is where the bounds come from. */
const SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  $defs: {
    AirQualityObserved: {
      properties: { pm10: {}, pm25: {}, location: {}, name: {} },
    },
  },
};

interface Options {
  blueprints?: unknown[];
  endpoints?: unknown[];
  schemaStatus?: number;
  write?: { body: unknown; status: number };
}

function renderGenerator(options: Options = {}) {
  const {
    blueprints = [BLUEPRINT_CARD],
    endpoints = [ENDPOINT],
    schemaStatus = 200,
    write = { body: CHANGE, status: 202 },
  } = options;

  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/api/v1/blueprints")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: blueprints });
    }
    if (url.pathname.endsWith("/endpoints") && request.method === "GET") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: endpoints });
    }
    if (url.pathname.includes("/schema/index.json")) {
      return json({ models: [{ name: "bb-air-quality", version: 2 }] }, schemaStatus);
    }
    if (url.pathname.includes("/schema/v2/json-schema")) {
      return json(SCHEMA, schemaStatus);
    }
    if (url.pathname.endsWith("/flows")) {
      return json(write.body, write.status);
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

/** The route a user takes to the form: the apps catalogue, then its own button. */
async function openGenerator(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: en.apps.newAction }));
}

/** The body of the one write the form makes. */
async function flowBody(fetchMock: ReturnType<typeof vi.fn>): Promise<Record<string, unknown>> {
  const request = fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .find((candidate) => candidate.method === "POST");
  expect(request).toBeDefined();
  return (await request!.json()) as Record<string, unknown>;
}

/** Fills the form the way a user would, and waits for the derived needs to arrive. */
async function fill(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText(en.apps.generate.name), "ovzdusie-dnes");
  await user.type(
    screen.getByLabelText(en.apps.generate.prompt),
    "A map of the stations with today's PM10",
  );
  await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint), "ovzdusie-public");
  await screen.findByText("AirQualityObserved");
}

describe("the app generator", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/apps`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("collects the prompt, the name and the kind", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.prompt);

    await user.type(screen.getByLabelText(en.apps.generate.prompt), "Show me the buses");

    expect(screen.getByLabelText(en.apps.generate.prompt)).toHaveValue("Show me the buses");
    // Fullstack is what the blueprint produces, so it is what the form starts on (AP-25).
    expect(screen.getByLabelText(en.apps.generate.kind)).toHaveValue("fullstack");
  });

  it("fills the endpoint list from the project's own endpoints", async () => {
    const user = userEvent.setup();
    renderGenerator({
      endpoints: [
        ENDPOINT,
        {
          ...ENDPOINT,
          metadata: { ...ENDPOINT.metadata, name: "doprava", title: { en: "Transport" } },
        },
      ],
    });
    await openGenerator(user);

    const select = await screen.findByLabelText(en.apps.generate.endpoint);
    const options = within(select).getAllByRole("option");

    expect(options.map((option) => option.textContent)).toEqual([
      en.apps.generate.pickEndpoint,
      "Air quality open data",
      "Transport",
    ]);
  });

  it("derives the readable attributes from the chosen endpoint's own schema (AP-22)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint), "ovzdusie-public");

    const needs = (await screen.findByText("AirQualityObserved")).closest(
      "fieldset",
    ) as HTMLElement;
    for (const attribute of ["pm10", "pm25", "location", "name"]) {
      expect(within(needs).getByLabelText(attribute)).toBeChecked();
    }
    // The endpoint's audience, not a field: an app is never wider than the data behind it.
    expect(
      screen.getByText(en.apps.generate.needs.audience.replace("{audience}", "public")),
    ).toBeInTheDocument();
  });

  it("sends the confirmed list, narrowed by what the user unticked", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);
    await fill(user);

    await user.click(screen.getByLabelText("pm25"));
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await waitFor(async () => {
      const body = await flowBody(fetchMock);
      expect(body.blueprint).toBe(BLUEPRINT);
      // The version the form was generated from, so the server can refuse a stale form.
      expect(body.version).toBe("1.4.0");
      const parameters = body.parameters as Record<string, unknown>;
      expect(parameters.name).toBe("ovzdusie-dnes");
      expect(parameters.kind).toBe("fullstack");
      expect(parameters.endpoint).toBe("ovzdusie-public");
      const needs = parameters.dataNeeds as { attrs: string[]; operations: string[] }[];
      expect(needs[0].attrs).toEqual(["location", "name", "pm10"]);
      expect(needs[0].operations).toEqual(["queryEntity", "retrieveEntity"]);
    });
  });

  it("shows the merge request the generation opened rather than a saved record", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    expect(await screen.findByText("chg-0000c3d4")).toBeInTheDocument();
    expect(screen.getByText(en.changes.accepted)).toBeInTheDocument();
  });

  it("names every rule the parameters broke instead of one", async () => {
    const user = userEvent.setup();
    renderGenerator({
      write: {
        status: 400,
        body: {
          type: "https://joinedcontext.com/errors/validation",
          title: "Bad Request",
          status: 400,
          detail: "the parameters do not satisfy the blueprint",
          errors: ["name: must be a DNS-1123 label", "prompt: must not be empty"],
        },
      },
    });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    expect(await screen.findByText("name: must be a DNS-1123 label")).toBeInTheDocument();
    expect(screen.getByText("prompt: must not be empty")).toBeInTheDocument();
  });

  it("cannot be submitted before an endpoint bounds it", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.name);

    await user.type(screen.getByLabelText(en.apps.generate.name), "ovzdusie-dnes");
    await user.type(screen.getByLabelText(en.apps.generate.prompt), "Anything at all");

    expect(screen.getByRole("button", { name: en.apps.generate.submit })).toBeDisabled();
  });

  it("says the endpoint publishes no model rather than generating against nothing", async () => {
    const user = userEvent.setup();
    renderGenerator({ schemaStatus: 406 });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint), "ovzdusie-public");

    expect(await screen.findByText(en.apps.generate.needs.unavailable)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.apps.generate.submit })).toBeDisabled();
  });

  it("explains itself and links the example apps when there is no builder (AG-26)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderGenerator({ blueprints: [] });
    await openGenerator(user);

    expect(await screen.findByText(en.apps.generate.noBuilder)).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: en.apps.generate.examples["hsl-transport"] }),
    ).toHaveAttribute("href", "/apps/hsl-transport/");
    expect(
      screen.getByRole("link", { name: en.apps.generate.examples["air-quality"] }),
    ).toHaveAttribute("href", "/apps/air-quality/");
    // No form means no way to open a merge request nothing would pick up.
    expect(screen.queryByLabelText(en.apps.generate.prompt)).not.toBeInTheDocument();
    expect(
      fetchMock.mock.calls.map((call) => call[0] as Request).some((r) => r.method === "POST"),
    ).toBe(false);
  });
});

describe("the derived grant", () => {
  const endpoint = ENDPOINT as never;
  const types = [{ name: "AirQualityObserved", attributes: ["location", "pm10", "pm25"] }];

  it("carries the endpoint's space and representations, and reads only", () => {
    const [need] = dataNeeds(endpoint, types, []);
    expect(need.contextSpaceRef).toEqual({ kind: "ContextSpace", name: "ovzdusie" });
    expect(need.representations).toEqual(["ngsi-ld", "geojson"]);
    expect(need.operations).toEqual(["queryEntity", "retrieveEntity"]);
  });

  it("drops a type whose every attribute the user unticked", () => {
    expect(
      dataNeeds(endpoint, types, [
        "AirQualityObserved.location",
        "AirQualityObserved.pm10",
        "AirQualityObserved.pm25",
      ]),
    ).toEqual([]);
  });

  it("can only narrow: an attribute the endpoint never published cannot appear", () => {
    const [need] = dataNeeds(endpoint, types, ["AirQualityObserved.pm25"]);
    expect(need.attrs).toEqual(["location", "pm10"]);
    expect(need.attrs).not.toContain("internalNote");
  });
});
