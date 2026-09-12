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
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2 } },
};

/** What `POST …/agent-runs` answers with: the queued run, plus the ticket only the caller sees. */
const CREATED_RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "queued",
  steps: 0,
  tokensUsed: 0,
  createdBy: IDENTITY.username,
  createdAt: "2026-09-12T08:00:00Z",
};

/** The endpoint's AuthZEN document, as the gateway's `/access` serves it to this user. */
const GRANT = {
  subject: { type: "user", id: "jana.kovacova" },
  permissions: [
    {
      resource: { type: "AirQualityObserved" },
      actions: ["queryEntity", "retrieveEntity"],
      attributes: ["location", "name", "pm10"],
    },
  ],
  prohibitions: [{ resource: { type: "AirQualityObserved" }, attributes: ["internalNote"] }],
};

/** Five entities as the gateway serves them with `options=keyValues`. */
const ENTITIES = [1, 2, 3, 4, 5].map((index) => ({
  id: `urn:ngsi-ld:AirQualityObserved:banskabystrica:ovzdusie:st-${index}`,
  type: "AirQualityObserved",
  name: `Station ${index}`,
  pm10: 12 + index,
}));

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
  runStatus?: string;
  grantStatus?: number;
  entities?: { body: unknown; status: number };
}

function renderGenerator(options: Options = {}) {
  const {
    blueprints = [BLUEPRINT_CARD],
    endpoints = [ENDPOINT],
    schemaStatus = 200,
    write = { body: CREATED_RUN, status: 202 },
    runStatus = "queued",
    grantStatus = 200,
    entities = { body: ENTITIES, status: 200 },
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
    if (url.pathname.endsWith("/access")) {
      return json(GRANT, grantStatus);
    }
    if (url.pathname.includes("/ngsi-ld/v1/entities")) {
      return json(entities.body, entities.status);
    }
    if (url.pathname.includes("/schema/index.json")) {
      return json({ models: [{ name: "bb-air-quality", version: 2 }] }, schemaStatus);
    }
    if (url.pathname.includes("/schema/v2/json-schema")) {
      return json(SCHEMA, schemaStatus);
    }
    if (url.pathname.endsWith("/agent-runs") && request.method === "POST") {
      return json(write.body, write.status);
    }
    if (url.pathname.endsWith("/publish")) {
      return json(CHANGE, 202);
    }
    if (url.pathname.includes("/agent-runs/") && request.method === "GET") {
      return json({ ...CREATED_RUN, status: runStatus });
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
async function runBody(fetchMock: ReturnType<typeof vi.fn>): Promise<Record<string, unknown>> {
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
  await screen.findByRole("group", { name: "AirQualityObserved" });
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

    const needs = await screen.findByRole("group", { name: "AirQualityObserved" });
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
      const body = await runBody(fetchMock);
      expect(body.appName).toBe("ovzdusie-dnes");
      expect(body.appClass).toBe("fullstack");
      expect(body.endpointName).toBe("ovzdusie-public");
      expect(body.prompt).toBe("A map of the stations with today's PM10");
      const needs = body.dataNeeds as { attrs: string[]; operations: string[] }[];
      expect(needs[0].attrs).toEqual(["location", "name", "pm10"]);
      expect(needs[0].operations).toEqual(["queryEntity", "retrieveEntity"]);
    });
  });

  it("opens the run it started rather than a saved record (AG-43)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);
    await fill(user);

    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    // The submit is not the end: the run is watched, and the review comes at publish time.
    expect(await screen.findByRole("heading", { name: "ovzdusie-dnes" })).toBeInTheDocument();
    expect(screen.getByText(en.agentRun.loginNote)).toBeInTheDocument();
    expect(screen.queryByLabelText(en.apps.generate.prompt)).not.toBeInTheDocument();
  });

  it("says a generated app is reachable only to a signed-in user (ADR-N-019)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);

    // The endpoint is public, and the app built on it still is not.
    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint), "ovzdusie-public");

    expect(await screen.findByText(en.apps.generate.needs.loginOnly)).toBeInTheDocument();
  });

  it("says in words what the endpoint lets this person read (AP-51)", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint), "ovzdusie-public");

    const preview = await screen.findByRole("region", { name: en.apps.generate.preview.title });
    expect(
      within(preview).getByText(
        en.apps.generate.preview.accessTypes.replace("{types}", "AirQualityObserved"),
      ),
    ).toBeInTheDocument();
    expect(
      within(preview).getByText(
        en.apps.generate.preview.accessAttrs.replace("{attrs}", "location, name, pm10"),
      ),
    ).toBeInTheDocument();
    // A prohibition is the one thing a person cannot infer from the checklist above.
    expect(
      within(preview).getByText(
        en.apps.generate.preview.denied.replace("{attrs}", "internalNote"),
      ),
    ).toBeInTheDocument();
    // A read grant is a read grant: nothing here promises the app could write.
    expect(within(preview).getByText(en.apps.generate.preview.readOnly)).toBeInTheDocument();
  });

  it("shows five entities as they are served, so the data can be judged before the build", async () => {
    const user = userEvent.setup();
    renderGenerator();
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);

    await user.selectOptions(screen.getByLabelText(en.apps.generate.endpoint), "ovzdusie-public");

    const table = await screen.findByRole("table", {
      name: en.apps.generate.preview.samplesTitle,
    });
    expect(within(table).getAllByRole("row")).toHaveLength(ENTITIES.length + 1);
    expect(within(table).getByText(ENTITIES[0].id)).toBeInTheDocument();
    expect(within(table).getByText("name=Station 1, pm10=13")).toBeInTheDocument();
  });

  it("still builds when the endpoint serves no samples and no grant", async () => {
    const user = userEvent.setup();
    renderGenerator({ grantStatus: 403, entities: { body: [], status: 200 } });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);
    await fill(user);

    const preview = screen.getByRole("region", { name: en.apps.generate.preview.title });
    expect(
      within(preview).getByText(en.apps.generate.preview.accessUnavailable),
    ).toBeInTheDocument();
    expect(within(preview).getByText(en.apps.generate.preview.samplesEmpty)).toBeInTheDocument();
    // The model is what bounds the app, so a silent `/access` is not a reason to stop.
    expect(screen.getByRole("button", { name: en.apps.generate.submit })).toBeEnabled();
  });

  it("publishing what was built opens a merge request like any other change (AP-55)", async () => {
    const user = userEvent.setup();
    renderGenerator({ runStatus: "previewing" });
    await openGenerator(user);
    await screen.findByLabelText(en.apps.generate.endpoint);
    await fill(user);
    await user.click(screen.getByRole("button", { name: en.apps.generate.submit }));

    await user.click(await screen.findByRole("button", { name: en.agentRun.publish }));

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
