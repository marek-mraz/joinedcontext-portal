/** T-0529: the explorer's "Your access" panel from the endpoint's AuthZEN surfaces (UI-33, R51, R17). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { deniedAttributes } from "../src/components/entities/AccessPanel";

const IDENTITY = { subject: "b7c1e0f4", username: "demo.viewer", name: "Demo Viewer", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    version: "1.0.0",
    linkml: [
      "id: https://hel.fi/models/bikes",
      "name: bikes",
      "classes:",
      "  BikeHireDockingStation:",
      "    slots: [id, availableBikeNumber, name, location]",
      "slots:",
      "  id: {}",
      "  availableBikeNumber: { range: integer }",
      "  name: { range: string }",
      "  location: { range: string }",
      "",
    ].join("\n"),
  },
};
const SPACES = list([
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "mobility", namespace: "helsinki" }, spec: { dataModelRef: "bikes" } },
]);
const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-bikes", namespace: "helsinki" },
    spec: { contextSpaceRef: "mobility", slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", audience: "public" },
  },
]);
/** The public grant on the bikes endpoint: reads only, two attributes, never the location. */
const GRANTS = {
  subject: { type: "role", id: "public" },
  resource: { type: "endpoint", id: "scsd2eehkx42n53z2zyd6vshfh7s7irf", space: "helsinki" },
  permissions: [
    {
      resource: { type: "BikeHireDockingStation" },
      actions: ["retrieveEntity", "queryEntity"],
      attributes: ["availableBikeNumber", "name", "location"],
      constraints: { scopeQ: "/helsinki/#" },
    },
  ],
  prohibitions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity"], attributes: ["location"], constraints: {} }],
};

function renderExplore() {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/access/check")) {
      return request
        .clone()
        .json()
        .then((body: { action: { name: string } }) => json({ decision: body.action.name === "queryEntity" }));
    }
    if (url.pathname.endsWith("/access")) return json(GRANTS);
    if (url.pathname.includes("/ngsi-ld/v1/entities")) return json([]);
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

describe("your access in the explorer", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/explore");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("renders the grant and the dry-run decisions, and disables a denied attribute with the reason", async () => {
    const fetchMock = renderExplore();
    await screen.findByRole("option", { name: "mobility" });
    await userEvent.selectOptions(screen.getByLabelText(en.explore.space), "mobility");
    const panel = await screen.findByRole("region", { name: en.access.panel.title });
    expect(within(panel).getByText(en.access.panel.chooseType)).toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText(en.entities.type), "BikeHireDockingStation");
    expect(await within(panel).findByText(/retrieveEntity, queryEntity/)).toBeInTheDocument();
    expect(within(panel).getByText(/scopeQ=\/helsinki\/#/)).toBeInTheDocument();
    // One check per action, with the user's session, answered before the query runs.
    expect(await within(panel).findByText("You may queryEntity BikeHireDockingStation")).toBeInTheDocument();
    expect(within(panel).getByText("You may not updateAttrs BikeHireDockingStation")).toBeInTheDocument();
    const checks = fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .filter((request) => request instanceof Request && request.url.endsWith("/access/check"));
    expect(checks).toHaveLength(2);
    expect(checks[0].credentials).toBe("same-origin");

    // The prohibited attribute cannot be asked for; the reason is on the checkbox.
    const location = screen.getByRole("checkbox", { name: `location: ${en.access.panel.attrProhibited}` });
    expect(location).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "name" })).toBeEnabled();
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "availableBikeNumber" })).toBeEnabled());
  });

  it("denies what is outside the grant's attribute list and nothing under a wildcard grant", () => {
    const t = (key: string) => key;
    const slots = [
      { name: "a", kind: "Property" as const },
      { name: "b", kind: "Property" as const },
    ];
    expect(deniedAttributes({ permissions: [{ resource: { type: "T" }, attributes: ["a"] }] }, "T", slots, t)).toEqual({
      b: "access.panel.attrDenied",
    });
    expect(deniedAttributes({ permissions: [{ resource: { type: "T" }, attributes: "*" }] }, "T", slots, t)).toEqual({});
    expect(deniedAttributes({ permissions: [{ resource: { type: "Other" }, attributes: ["a"] }] }, "T", slots, t)).toEqual({});
    expect(deniedAttributes(undefined, "T", slots, t)).toEqual({});
  });
});
