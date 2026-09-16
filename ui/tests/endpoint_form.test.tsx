import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { stringify as stringifyYaml } from "yaml";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { rememberPrefill } from "../src/assistant/state";
import { App } from "../src/App";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom: the stand-in
// is a textarea with the same contract, so the YAML view's own work is what runs.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const ORG = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Organization",
      metadata: { name: "bb", namespace: "banskabystrica" },
      spec: { domain: "hel.fi" },
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

const LINKML_SOURCE = `
id: https://hel.fi/models/mobility
name: mobility
classes:
  Vehicle:
    slots:
      - id
      - name
      - type
      - location
      - speed
  User:
    slots:
      - id
      - name
      - type
      - age
      - email
slots:
  id:
    range: string
  type:
    range: string
  name:
    range: string
  location:
    range: string
  speed:
    range: integer
  age:
    range: integer
  email:
    range: string
`;

const DATAMODELS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "mobility-model", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        version: "1",
        linkml: LINKML_SOURCE,
      },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-test-123", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 3 } },
};

const EXISTING = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "vehicles-live", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
        audience: "organization",
        enabledRepresentations: ["ngsi-ld"],
        projectionRef: { kind: "ModelProjection", name: "vehicles" },
      },
      status: { phase: "Live" },
    },
  ],
};

function setupTest(endpoints: unknown = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }) {
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

    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/organizations")) return json(ORG);
    if (url.pathname.endsWith("/spaces")) return json(SPACES);
    if (url.pathname.endsWith("/datamodels")) return json(DATAMODELS);
    if (url.pathname.endsWith("/projections")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    if (url.pathname.endsWith("/endpoints") && request.method === "POST") return json({ valid: true, lane: "yellow" });
    if (url.pathname.endsWith("/endpoints/vehicles-live")) return json(CHANGE, 202);
    if (url.pathname.endsWith("/endpoints")) return json(endpoints);
    if (url.pathname.endsWith("/import")) return json(CHANGE, 202);

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

describe("endpoint form with ModelPicker (T-0564)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows read-only slug and public URL without slug input or generate button", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));

    expect(screen.getByTestId("endpoint-slug")).toBeInTheDocument();
    expect(screen.getByTestId("endpoint-url")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.endpoints.generateSlug })).not.toBeInTheDocument();
  });

  it("ticking classes and slots posts bundle to /import with ModelProjection, Endpoint and Policies", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));

    const dialog = await screen.findByRole("dialog");
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "public-vehicles");

    // Wait for ModelPicker
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());

    // Tick Vehicle (ticks id, type, name, location, speed)
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));
    // Untick location, speed
    await userEvent.click(within(dialog).getByLabelText("Vehicle.location"));
    await userEvent.click(within(dialog).getByLabelText("Vehicle.speed"));

    // Tick User
    await userEvent.click(within(dialog).getByLabelText("User"));
    // Untick email
    await userEvent.click(within(dialog).getByLabelText("User.email"));

    // Identity slots id, type must be checked & disabled
    expect(within(dialog).getByLabelText("Vehicle.id")).toBeChecked();
    expect(within(dialog).getByLabelText("Vehicle.id")).toBeDisabled();
    expect(within(dialog).getByLabelText("Vehicle.type")).toBeChecked();
    expect(within(dialog).getByLabelText("Vehicle.type")).toBeDisabled();

    // Click propose
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => {
      const importCalls = fetchMock.mock.calls.filter((c) => {
        const req = c[0] as Request;
        const u = typeof c[0] === "string" ? c[0] : req.url;
        return u.includes("/import");
      });
      expect(importCalls.length).toBe(1);
    });

    const importCall = fetchMock.mock.calls.find((c) => {
      const req = c[0] as Request;
      const u = typeof c[0] === "string" ? c[0] : req.url;
      return u.includes("/import");
    })![0] as Request;

    const payload = (await importCall.clone().json()) as { manifests: Record<string, unknown>[] };
    expect(payload.manifests).toBeDefined();
    expect(payload.manifests).toHaveLength(3); // ModelProjection, Endpoint, Read Policy

    const [proj, ep, readPol] = payload.manifests;
    expect(proj.kind).toBe("ModelProjection");
    expect((proj.spec as Record<string, unknown>).classes).toEqual([
      { name: "Vehicle", slots: ["name"] },
      { name: "User", slots: ["name", "age"] },
    ]);

    expect(ep.kind).toBe("Endpoint");
    expect((ep.spec as Record<string, unknown>).projectionRef).toEqual({
      kind: "ModelProjection",
      name: "public-vehicles",
    });

    expect(readPol.kind).toBe("Policy");
    expect((readPol.spec as Record<string, unknown>).operations).toEqual(["retrieveOps"]);
    expect((readPol.spec as Record<string, unknown>).information).toEqual([
      { entities: [{ type: "Vehicle" }], propertyNames: ["name"] },
      { entities: [{ type: "User" }], propertyNames: ["name", "age"] },
    ]);
  });

  it("refuses proposal if space has model but nothing is ticked", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "empty-vehicles");

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    expect(await within(dialog).findByText(en.endpoints.picker.nothingTicked)).toBeInTheDocument();
  });

  it("ticks the classes the assistant's proposal names, so its check is not refused (T-0895)", async () => {
    window.history.replaceState(null, "", "/projects/banskabystrica/endpoints");
    rememberPrefill("/projects/banskabystrica/endpoints", {
      name: "vehicles-regional",
      contextSpaceRef: "ovzdusie",
      audience: "project-list",
      allowedProjects: ["regional-transport"],
      enabledRepresentations: ["ngsi-ld"],
      hiddenAttributes: ["speed"],
      entityTypes: ["Vehicle"],
    });
    setupTest();
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeChecked());
    expect(within(dialog).getByLabelText("Vehicle.speed")).toBeChecked();
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));
    await waitFor(() => expect(within(dialog).queryByText(en.endpoints.picker.nothingTicked)).not.toBeInTheDocument());
  });

  it("class ticked with no slots is identity only and shows text", async () => {
    setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    // Untick all non-identity slots: name, location, speed
    await userEvent.click(within(dialog).getByLabelText("Vehicle.name"));
    await userEvent.click(within(dialog).getByLabelText("Vehicle.location"));
    await userEvent.click(within(dialog).getByLabelText("Vehicle.speed"));

    expect(within(dialog).getByText(en.endpoints.picker.identityOnly, { exact: false })).toBeInTheDocument();
  });

  it("writable class with idPattern and scope creates write policy", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "write-vehicles");

    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    // Mark writable
    await userEvent.click(within(dialog).getByLabelText("Vehicle writable"));
    await userEvent.type(
      within(dialog).getByLabelText("Vehicle idPattern"),
      "urn:ngsi-ld:Vehicle:hel.fi:helsinki:*",
    );
    await userEvent.type(within(dialog).getByLabelText("Vehicle scope"), "/helsinki/bikes");

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => {
      const importCalls = fetchMock.mock.calls.filter((c) => {
        const req = c[0] as Request;
        const u = typeof c[0] === "string" ? c[0] : req.url;
        return u.includes("/import");
      });
      expect(importCalls.length).toBe(1);
    });

    const importCall = fetchMock.mock.calls.find((c) => {
      const req = c[0] as Request;
      const u = typeof c[0] === "string" ? c[0] : req.url;
      return u.includes("/import");
    })![0] as Request;

    const payload = (await importCall.clone().json()) as { manifests: Record<string, unknown>[] };
    expect(payload.manifests).toHaveLength(4); // ModelProjection, Endpoint, Read Policy, Write Policy
    const writePol = payload.manifests[3];
    expect(writePol.kind).toBe("Policy");
    expect((writePol.spec as Record<string, unknown>).operations).toEqual(["updateOps"]);
    expect((writePol.spec as Record<string, unknown>).scopeQ).toBe("/helsinki/bikes");
    expect((writePol.spec as Record<string, unknown>).information).toEqual([
      {
        entities: [
          { type: "Vehicle", idPattern: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:*" },
        ],
      },
    ]);
  });

  // T-0763: writes of the page other than its drafts, as "METHOD path?query".
  const writes = (fetchMock: ReturnType<typeof setupTest>) =>
    fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .filter((request) => request.method !== "GET" && !request.url.includes("/drafts"))
      .map((request) => ({ request, line: `${request.method} ${new URL(request.url).pathname}${new URL(request.url).search}` }));

  it("checks and proposes an existing endpoint alone, with the projection it names and no class ticked (T-0763)", async () => {
    const fetchMock = setupTest(EXISTING);
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.edit }));
    const dialog = await screen.findByRole("dialog");

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [check] = writes(fetchMock);
    expect(check.line).toBe("POST /api/v1/projects/banskabystrica/endpoints?dryRun=All");
    await expect(check.request.clone().json()).resolves.toMatchObject({
      draft: { kind: "Endpoint", name: "vehicles-live" },
      spec: { projectionRef: { kind: "ModelProjection", name: "vehicles" } },
    });

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(2));
    expect(writes(fetchMock)[1].line).toBe("PUT /api/v1/projects/banskabystrica/endpoints/vehicles-live");
    expect(writes(fetchMock).some((write) => write.line.includes("/import"))).toBe(false);
  });

  it("proposes a rate limit typed in the YAML view outside the classes, as the REST route would (T-0890)", async () => {
    const stored = EXISTING.items[0];
    const limited = { ...stored, spec: { ...stored.spec, rateLimits: { requestsPerMinute: 600, burst: 50 } } };
    const fetchMock = setupTest({ ...EXISTING, items: [limited] });
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.edit }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("tab", { name: "YAML" }));
    const editor = await within(dialog).findByLabelText("YAML");
    // 300 is no class: the form's select offers 60, 600 and 6000, plus what is stored.
    const pasted = { ...limited, status: undefined, spec: { ...limited.spec, rateLimits: { requestsPerMinute: 300, burst: 50 } } };
    fireEvent.change(editor, { target: { value: stringifyYaml(pasted) } });
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    expect(writes(fetchMock)[0].line).toBe("PUT /api/v1/projects/banskabystrica/endpoints/vehicles-live");
    const body = (await writes(fetchMock)[0].request.clone().json()) as {
      spec: { rateLimits?: { requestsPerMinute?: number } };
    };
    expect(body.spec.rateLimits?.requestsPerMinute).toBe(300);
    expect(within(dialog).queryByText(en.form.invalid)).toBeNull();
  });

  it("checks a new endpoint's bundle whole and then its endpoint with its draft, so the verdict is the form's (T-0763)", async () => {
    const fetchMock = setupTest();
    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "public-vehicles");
    await waitFor(() => expect(within(dialog).getByLabelText("Vehicle")).toBeInTheDocument());
    await userEvent.click(within(dialog).getByLabelText("Vehicle"));

    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(2));
    const [bundle, endpoint] = writes(fetchMock);
    expect(bundle.line).toBe("POST /api/v1/projects/banskabystrica/import?dryRun=All");
    expect(endpoint.line).toBe("POST /api/v1/projects/banskabystrica/endpoints?dryRun=All");
    await expect(endpoint.request.clone().json()).resolves.toMatchObject({
      draft: { kind: "Endpoint", name: "public-vehicles" },
      spec: { projectionRef: { kind: "ModelProjection", name: "public-vehicles" } },
    });
  });
});
