import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import validator from "@rjsf/validator-ajv8";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import {
  admits,
  dns1123,
  referenceManifest,
  sharedSpaceReferenceSchema,
} from "../src/components/endpoints/sharing";
import type { Manifest } from "../src/api/manifest";

/**
 * T-0498: sharing across the projects of one organization (EP-14, EP-15, PF-05, UI-01):
 * the owner picks the allowed projects from the project list and sees who an endpoint is
 * shared with; the consumer sees what the other projects opened to it and references it
 * with one click, through the same write API as every other change.
 */

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

const API = "joinedcontext.com/v1alpha1";
const BIKES_SLUG = "abcdefghijklmnopqrstuvwxyz";
const EVENTS_SLUG = "zyxwvutsrqponmlkjihgfedcba";
const WEATHER_SLUG = "q2w3e4r5t6y7u2i3o4p5a6s7d2";
const AIR_SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

function endpoint(
  project: string,
  space: string,
  name: string,
  slug: string,
  audience: string,
  allowedProjects?: string[],
) {
  return {
    apiVersion: API,
    kind: "Endpoint",
    metadata: { name, namespace: project, labels: { "joinedcontext.com/space": space } },
    spec: {
      contextSpaceRef: space,
      slug,
      audience,
      enabledRepresentations: ["ngsi-ld", "geojson"],
      ...(allowedProjects ? { allowedProjects } : {}),
    },
    status: { phase: "Live" },
  };
}

function list(items: unknown[]) {
  return { apiVersion: API, kind: "List", items };
}

const CHANGE = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "espoo" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

/** Three projects of the same organization: helsinki publishes, espoo consumes, vantaa is a bystander. */
function responses(): Record<string, unknown> {
  return {
    "/api/v1/projects": list([{ name: "helsinki" }, { name: "espoo" }, { name: "vantaa" }]),
    "/api/v1/projects/helsinki/spaces": list([
      { apiVersion: API, kind: "ContextSpace", metadata: { name: "liikenne", namespace: "helsinki" }, spec: {} },
    ]),
    "/api/v1/projects/helsinki/endpoints": list([
      endpoint("helsinki", "liikenne", "bikes", BIKES_SLUG, "project-list", ["espoo"]),
      endpoint("helsinki", "liikenne", "events", EVENTS_SLUG, "organization"),
      endpoint("helsinki", "liikenne", "weather", WEATHER_SLUG, "project-list", ["vantaa"]),
    ]),
    "/api/v1/projects/vantaa/endpoints": list([
      endpoint("vantaa", "ilma", "air", AIR_SLUG, "public"),
    ]),
    "/api/v1/projects/espoo/endpoints": list([]),
    "/api/v1/projects/espoo/shared": list([]),
  };
}

let byPath: Record<string, unknown>;

function renderAt(path: string) {
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
    if (request.method !== "GET") {
      return json(CHANGE, 202);
    }
    return json(byPath[url.pathname] ?? list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
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

function sharedSection(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: en.endpoints.shared.title });
}

describe("endpoint sharing", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    byPath = responses();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("admits a project the way the gateway does", () => {
    const listed = endpoint("helsinki", "liikenne", "bikes", BIKES_SLUG, "project-list", ["espoo"]) as Manifest;
    const org = endpoint("helsinki", "liikenne", "events", EVENTS_SLUG, "organization") as Manifest;
    const open = endpoint("vantaa", "ilma", "air", AIR_SLUG, "public") as Manifest;
    expect(admits(listed, "helsinki", "espoo")).toBe(true);
    expect(admits(listed, "helsinki", "vantaa")).toBe(false);
    expect(admits(listed, "helsinki", "helsinki")).toBe(true);
    expect(admits(org, "helsinki", "vantaa")).toBe(true);
    expect(admits(open, "vantaa", "espoo")).toBe(true);
  });

  it("picks the allowed projects from the project list, without the project itself", async () => {
    const fetchMock = renderAt("/projects/helsinki/endpoints");

    const row = (await screen.findByText("bikes")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.endpoints.edit }));
    const dialog = await screen.findByRole("dialog");

    // A checkbox per other project, the one the manifest lists already ticked; no free text.
    const espoo = await within(dialog).findByRole("checkbox", { name: "espoo" });
    expect(espoo).toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "vantaa" })).not.toBeChecked();
    expect(within(dialog).queryByRole("checkbox", { name: "helsinki" })).not.toBeInTheDocument();
    expect(dialog.querySelector("#root_allowedProjects_0")).toBeNull();

    await userEvent.click(within(dialog).getByRole("checkbox", { name: "vantaa" }));
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/helsinki/endpoints/bikes");
    const body = (await request.clone().json()) as { spec: { allowedProjects?: string[] } };
    expect(body.spec.allowedProjects).toEqual(["espoo", "vantaa"]);
  });

  it("shows the owner who each endpoint is shared with", async () => {
    renderAt("/projects/helsinki/endpoints");

    const bikes = (await screen.findByText("bikes")).closest("tr") as HTMLElement;
    const sharedWith = within(bikes).getByRole("list", { name: en.endpoints.sharedWith });
    expect(within(sharedWith).getByText("espoo")).toBeInTheDocument();
    expect(within(bikes).getByText(en.endpoints.audience["project-list"])).toBeInTheDocument();

    const events = screen.getByText("events").closest("tr") as HTMLElement;
    expect(within(events).getByText(en.endpoints.audience.organization)).toBeInTheDocument();
    expect(
      within(events).queryByRole("list", { name: en.endpoints.sharedWith }),
    ).not.toBeInTheDocument();
  });

  it("lists for the consumer only the endpoints whose audience admits it", async () => {
    renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    // bikes lists espoo, events is organization-wide, air is public: all three are admitted.
    await within(section).findByText("bikes");
    expect(within(section).getByText("events")).toBeInTheDocument();
    expect(within(section).getByText("air")).toBeInTheDocument();
    // weather lists vantaa only: espoo may not see it.
    expect(within(section).queryByText("weather")).not.toBeInTheDocument();

    const bikes = within(section).getByText("bikes").closest("tr") as HTMLElement;
    expect(within(bikes).getByRole("link", { name: "helsinki" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints",
    );
    expect(within(bikes).getByRole("link", { name: "liikenne" })).toHaveAttribute(
      "href",
      "/projects/helsinki/spaces/liikenne",
    );
    expect(within(bikes).getByRole("link", { name: "ngsi-ld" })).toHaveAttribute(
      "href",
      `${window.location.origin}/api/endpoint/${BIKES_SLUG}/ngsi-ld/v1/entities?limit=20`,
    );
  });

  it("references a shared endpoint with one click through the write API", async () => {
    const fetchMock = renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    await within(section).findByText("bikes");
    await userEvent.click(
      within(section).getByRole("button", { name: `${en.endpoints.shared.use}: helsinki/bikes` }),
    );

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/espoo/shared");
    const body = (await request.clone().json()) as {
      kind: string;
      metadata: { name: string; namespace: string };
      spec: { endpointSlug: string; alias: string };
    };
    expect(body).toEqual({
      apiVersion: API,
      kind: "SharedSpaceReference",
      metadata: { name: "helsinki-bikes", namespace: "espoo" },
      spec: { endpointSlug: BIKES_SLUG, alias: "helsinki-liikenne" },
    });
    // The spec is what jc-core's `SharedSpaceReferenceSpec` accepts, field for field.
    expect(validator.isValid(sharedSpaceReferenceSchema, body.spec, sharedSpaceReferenceSchema)).toBe(
      true,
    );
    // The write is a Change in the approvals flow, and the page says so.
    expect(await screen.findByText("chg-77aa11bb")).toBeInTheDocument();
  });

  it("shows a reference already declared with its alias instead of the button", async () => {
    byPath["/api/v1/projects/espoo/shared"] = list([
      {
        apiVersion: API,
        kind: "SharedSpaceReference",
        metadata: { name: "helsinki-bikes", namespace: "espoo" },
        spec: { endpointSlug: BIKES_SLUG, alias: "city-bikes" },
        status: { phase: "Live" },
      },
    ]);
    renderAt("/projects/espoo/endpoints");

    const section = await sharedSection();
    const bikes = (await within(section).findByText("bikes")).closest("tr") as HTMLElement;
    await within(bikes).findByText(en.endpoints.shared.referenced);
    expect(within(bikes).getByText(/city-bikes/)).toBeInTheDocument();
    expect(
      within(bikes).queryByRole("button", { name: `${en.endpoints.shared.use}: helsinki/bikes` }),
    ).not.toBeInTheDocument();
    // The other endpoints still offer the button.
    const events = within(section).getByText("events").closest("tr") as HTMLElement;
    expect(
      within(events).getByRole("button", { name: `${en.endpoints.shared.use}: helsinki/events` }),
    ).toBeInTheDocument();
  });

  it("names the reference after the source so it never shadows a local space", () => {
    const bikes = endpoint("helsinki", "liikenne", "bikes", BIKES_SLUG, "organization") as Manifest;
    expect(referenceManifest("espoo", "helsinki", bikes).spec.alias).toBe("helsinki-liikenne");
    expect(dns1123("Helsinki", "City Bikes!")).toBe("helsinki-city-bikes");
    expect(dns1123("a".repeat(40), "b".repeat(40))).toHaveLength(63);
  });

  it("lists the shared references as their own section of the sidebar", async () => {
    renderAt("/projects/espoo/endpoints");

    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    expect(within(nav).getByRole("link", { name: en.nav.shared })).toHaveAttribute(
      "href",
      "/projects/espoo/shared",
    );
  });
});
