/**
 * One endpoint, the whole window (T-2281, EP-51, UI-26, UI-44, UI-61).
 *
 * 1. Every section is on the page, with the value the manifest carries: the space, the audience and
 *    the allowed projects, the public address, the representations, the hidden attributes, the
 *    policy, the rate limit, the cache lifetime and the file limits.
 * 2. The filtering section reads the `q`, the `scopeQ`, the `geoQ` and the `temporalQ` of the named
 *    projection the endpoint publishes through — an endpoint carries none of its own (MP-01) — and
 *    says so with the projection's name.
 * 3. An endpoint with no projection says the whole space is published and how to narrow it, instead
 *    of showing four empty fields.
 * 4. "Change these settings" goes to the endpoints list with `?edit=<name>`, which opens the kind's
 *    own editor on that endpoint — one form, not a second copy of it.
 * 5. A viewer reads all of it and cannot propose: the control is disabled with the reason, and the
 *    Delete in the menu keeps its reason too (UI-44).
 * 6. The page says a change is proposed and reviewed before any control.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const PROJECT = "banskabystrica";
const NAME = "ovzdusie-public";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: {
    name: NAME,
    namespace: PROJECT,
    title: "Air quality, open",
    labels: { "joinedcontext.com/space": "ovzdusie" },
  },
  spec: {
    contextSpaceRef: "ovzdusie",
    slug: SLUG,
    audience: "project-list",
    allowedProjects: ["kosice"],
    policyRef: "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie:readers",
    enabledRepresentations: ["ngsi-ld", "geojson"],
    rateLimits: { requestsPerMinute: 120, burst: 20 },
    caching: { maxAgeSeconds: 60 },
    fileLimits: { maxFileRows: 50000 },
    projection: { hiddenAttributes: ["reportedBy"] },
    projectionRef: { kind: "ModelProjection", name: "ovzdusie-open" },
  },
  status: { phase: "Live" },
};

const PROJECTION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ModelProjection",
  metadata: { name: "ovzdusie-open", namespace: PROJECT },
  spec: {
    contextSpaceRef: "ovzdusie",
    dataModelRef: "ovzdusie",
    classes: [{ name: "AirQualityObserved", slots: ["pm10"] }],
    filter: { q: "pm10>30", scopeQ: "/Banska Bystrica/Centrum" },
  },
};

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

const PERMITTED = {
  project: PROJECT,
  bootstrap: false,
  grants: [
    {
      role: "endpoint-editor",
      binding: "editors",
      rule: { kinds: ["Endpoint"], verbs: ["propose", "delete"] },
    },
  ],
};

/** A viewer: they read everything and propose nothing (UI-44). */
const VIEWER = { project: PROJECT, bootstrap: false, grants: [] };

function renderPage({
  endpoint = ENDPOINT,
  projections = [PROJECTION],
  permissions = PERMITTED,
}: {
  endpoint?: unknown;
  projections?: unknown[];
  permissions?: unknown;
} = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
    const path = url.pathname;
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
    if (path.endsWith("/permissions/me")) {
      return json(permissions);
    }
    if (path.endsWith(`/endpoints/${NAME}`)) {
      return endpoint === null ? json({ title: "Not Found", detail: "no such endpoint" }, 404) : json(endpoint);
    }
    if (path.endsWith("/projections")) {
      return json(list(projections));
    }
    if (path.endsWith("/endpoints")) {
      return json(list(endpoint === null ? [] : [endpoint]));
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

describe("the endpoint's own settings page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/endpoints/${NAME}`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("holds every setting the manifest carries, one section per aspect", async () => {
    renderPage();

    expect(await screen.findByRole("heading", { name: "Air quality, open" })).toBeInTheDocument();
    for (const section of [
      en.endpoints.page.whatItIs,
      en.endpoints.page.whatItAnswers,
      en.endpoints.page.filtering,
      en.endpoints.page.whoMayCall,
      en.endpoints.page.traffic,
    ]) {
      expect(screen.getByRole("heading", { name: section })).toBeInTheDocument();
    }

    // What it is: the space it publishes, who it admits, and the address a caller uses.
    expect(screen.getByRole("link", { name: "ovzdusie" })).toBeInTheDocument();
    expect(screen.getByText(en.endpoints.audience["project-list"])).toBeInTheDocument();
    expect(screen.getByText("kosice")).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/api/endpoint/${SLUG}`)).toBeInTheDocument();

    // What it answers, and what it keeps back.
    expect(screen.getByRole("link", { name: "ngsi-ld" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "geojson" })).toBeInTheDocument();
    expect(screen.getByText("AirQualityObserved")).toBeInTheDocument();
    expect(screen.getByText("reportedBy")).toBeInTheDocument();

    // Who may call it, and what it limits.
    expect(screen.getByText(ENDPOINT.spec.policyRef)).toBeInTheDocument();
    expect(screen.getByText("120 requests a minute · burst 20")).toBeInTheDocument();
    expect(screen.getByText("A caller may cache an answer for 60 seconds.")).toBeInTheDocument();
    expect(screen.getByText("At most 50000 rows in a file")).toBeInTheDocument();

    // A change is proposed and reviewed, said before the first control (PF-57).
    expect(screen.getByText(en.endpoints.page.proposedNotice)).toBeInTheDocument();
  });

  it("reads the filtering from the named projection it publishes through", async () => {
    renderPage();

    expect(await screen.findByRole("heading", { name: en.endpoints.page.filtering })).toBeInTheDocument();
    expect(screen.getByText("pm10>30")).toBeInTheDocument();
    expect(screen.getByText("/Banska Bystrica/Centrum")).toBeInTheDocument();
    // Every condition says what it narrows, and an empty one says it narrows nothing.
    expect(screen.getByText(en.endpoints.filter.qHelp)).toBeInTheDocument();
    expect(screen.getAllByText(en.endpoints.page.filterEmpty)).toHaveLength(2);
    expect(screen.getByRole("link", { name: "ovzdusie-open" })).toBeInTheDocument();
  });

  it("says the whole space is published when there is no projection to filter with", async () => {
    const spec = { ...ENDPOINT.spec };
    delete (spec as { projectionRef?: unknown }).projectionRef;
    renderPage({ endpoint: { ...ENDPOINT, spec } });

    expect(await screen.findByText(en.endpoints.page.noProjection)).toBeInTheDocument();
    expect(screen.queryByText(en.endpoints.filter.qHelp)).not.toBeInTheDocument();
    // Without a projection the types are the space's own, not an empty list pretending to be none.
    expect(screen.getByText(en.endpoints.page.everyTypeOfTheSpace)).toBeInTheDocument();
  });

  it("opens the kind's own editor on this endpoint through ?edit=", async () => {
    renderPage();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: en.endpoints.page.change }));

    await waitFor(() => {
      expect(window.location.search).toBe(`?edit=${NAME}`);
    });
    // The list's editor opens on that endpoint: its name is in the form, not an empty one.
    expect(await screen.findByDisplayValue(NAME)).toBeInTheDocument();
  });

  it("lets a viewer read every setting and refuses the change with its reason", async () => {
    renderPage({ permissions: VIEWER });

    expect(await screen.findByRole("heading", { name: "Air quality, open" })).toBeInTheDocument();
    expect(screen.getByText("pm10>30")).toBeInTheDocument();

    // The guard remounts the control once the document arrives, so it is queried after the wait.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.endpoints.page.change })).toBeDisabled();
    });
    expect(screen.getByTitle(/does not permit/)).toBeInTheDocument();
  });

  it("keeps the rest of the actions in one menu, and a denied one with its reason", async () => {
    renderPage({ permissions: VIEWER });
    const user = userEvent.setup();

    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.endpoints.page.change })).toBeDisabled();
    });
    await user.click(screen.getByRole("button", { name: /More actions for/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: en.endpoints.copyUrl })).toBeInTheDocument();
    // The accessible name carries the reason too, so the item is matched by pattern (T-2279).
    const remove = within(menu).getByRole("menuitem", { name: /Delete/ });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    expect(remove).toHaveAttribute("title", expect.stringContaining("delete"));
  });

  it("says why an endpoint could not be read instead of an empty page", async () => {
    renderPage({ endpoint: null });

    expect(await screen.findByRole("alert")).toHaveTextContent("no such endpoint");
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });
});
