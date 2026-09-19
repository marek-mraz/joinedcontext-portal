import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { generateSlug, SLUG_PATTERN } from "../src/schemas/kinds";
import { rememberPrefill } from "../src/assistant/state";
import { greenVerdict, isCheck } from "./verdict";

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

/** The published schema of the endpoint above: one class, and an attribute a steward may hide. */
const PUBLISHED = {
  $defs: {
    AirQualityObserved: { properties: { pm10: {}, sensorSerial: {} } },
  },
};

const AIR_ROWS = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:radvan-01",
    type: "AirQualityObserved",
    pm10: { type: "Property", value: 12, unitCode: "GQ" },
    sensorSerial: { type: "Property", value: "SN-9" },
  },
];

/** One call, whichever way it was made: the app sends a `Request`, the SDK grid a path (T-1433). */
function callOf(input: unknown, init?: RequestInit): { url: URL; method: string } {
  const request = input instanceof Request ? input : undefined;
  const raw = request?.url ?? String(input);
  return {
    url: new URL(raw, window.location.origin),
    method: request?.method ?? init?.method ?? "GET",
  };
}

function renderEndpoints(permissions?: unknown, entities?: () => Response) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const request = input as Request;
    const { url, method } = callOf(input, init);
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
    if (permissions !== undefined && path.endsWith("/permissions/me")) {
      return json(permissions);
    }
    // The check is a dry run: it answers a verdict and writes nothing (AG-62, PF-57).
    if (isCheck(request, url)) {
      return greenVerdict(request, { valid: true, lane: "yellow" }).then((body) => json(body));
    }
    // What the endpoint publishes of its model, and what it answers (T-1433).
    if (path.endsWith("/schema/index.json")) {
      return json({ models: [{ name: "bb-air-quality", version: 1 }] });
    }
    if (path.endsWith("/schema/v1/json-schema")) {
      return json(PUBLISHED);
    }
    if (path.includes("/ngsi-ld/v1/entities")) {
      return Promise.resolve(
        entities
          ? entities()
          : new Response(JSON.stringify(AIR_ROWS), {
              status: 200,
              headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
            }),
      );
    }
    if (method !== "GET") {
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
    .filter((call) => call[0] instanceof Request)
    .map((call) => call[0] as Request)
    .filter((request) => request.method !== "GET" && !request.url.includes("/drafts"));
}

/** The writes that are not the check's dry run: what the person actually proposed. */
function proposals(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return writes(fetchMock).filter((request) => !new URL(request.url).searchParams.has("dryRun"));
}

/** The row's actions live behind its one menu now (T-2287): open it and hand back the item. */
async function rowMenuItem(row: HTMLElement, name: string | RegExp) {
  await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
  return screen.findByRole("menuitem", { name });
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

  it("shows a viewer the edit and the delete of an endpoint disabled with the reason (UI-44)", async () => {
    renderEndpoints({
      project: "banskabystrica",
      bootstrap: false,
      grants: [{ role: "viewer", binding: "viewers", rule: { kinds: ["Endpoint"], verbs: ["read"] } }],
    });

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await waitFor(() => {
      expect(within(row).getByRole("button", { name: /More actions/ })).toBeInTheDocument();
    });
    // Disabled with the reason inside the menu: a role too narrow has to be readable, not absent.
    const edit = await rowMenuItem(row, new RegExp(en.endpoints.edit));
    expect(edit).toHaveAttribute("aria-disabled", "true");
    expect(edit).toHaveAttribute(
      "title",
      "Disabled: your role does not permit 'propose' on 'Endpoint' in this project",
    );
    const menu = screen.getByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: /Delete/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("keeps one control in the open and the rest in the row's menu (T-2287)", async () => {
    renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    // What it answers stays out, because that is why a person opens this table; the menu button is
    // the only other control in the cell.
    const cell = row.lastElementChild as HTMLElement;
    const open = within(cell).getAllByRole("button");
    expect(open.map((button) => button.textContent)).toEqual([en.endpoints.data.action, "\u22ef"]);

    await userEvent.click(within(cell).getByRole("button", { name: /More actions/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      en.endpoints.edit,
      en.endpoints.copyUrl,
      en.export.action,
      en.saveAs.button,
      en.workspaces.open.action,
      en.resourceDelete.button,
    ]);

    // Escape closes it and the focus comes back to the button that opened it.
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
    expect(within(cell).getByRole("button", { name: /More actions/ })).toHaveFocus();
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
    await userEvent.click(await rowMenuItem(row, en.endpoints.copyUrl));

    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/api/endpoint/${SLUG}`);
  });

  it("shows read-only slug and public URL for a new endpoint", async () => {
    renderEndpoints();

    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");
    const slugCode = within(dialog).getByTestId("endpoint-slug");
    expect(slugCode.textContent).toMatch(new RegExp(SLUG_PATTERN));
    const urlCode = within(dialog).getByTestId("endpoint-url");
    expect(urlCode.textContent).toContain(slugCode.textContent!);
  });

  it("proposes an added representation on an existing endpoint (DEMO step 7)", async () => {
    const fetchMock = renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(await rowMenuItem(row, en.endpoints.edit));

    const dialog = await screen.findByRole("dialog");
    // The person reads what a representation is; the manifest keeps the contract's word.
    expect(
      within(dialog).getByRole("checkbox", { name: en.endpoints.representationOption["ngsi-ld"] }),
    ).toBeChecked();
    const csv = within(dialog).getByRole("checkbox", { name: en.endpoints.representationOption.csv });
    expect(csv).not.toBeChecked();
    await userEvent.click(csv);

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: en.endpoints.propose })).toBeEnabled(),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(proposals(fetchMock)).toHaveLength(1));
    const request = proposals(fetchMock)[0];
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

  it("edits the endpoint the assistant opened: a PUT to its name that keeps its slug", async () => {
    rememberPrefill("/projects/banskabystrica/endpoints", {
      existing: true,
      name: "public-air",
      contextSpaceRef: "ovzdusie",
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "geojson", "csv"],
      slug: SLUG,
    });
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints?draft=public-air");
    const fetchMock = renderEndpoints();

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: en.endpoints.edit })).toBeInTheDocument();
    await waitFor(() => expect(within(dialog).getByTestId("endpoint-slug").textContent).toBe(SLUG));
    expect(
      within(dialog).getByRole("checkbox", { name: en.endpoints.representationOption.csv }),
    ).toBeChecked();

    // Opened on the assistant's change, the page checks what the form shows before anything else.
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const checked = writes(fetchMock)[0];
    expect(`${checked.method} ${new URL(checked.url).pathname}${new URL(checked.url).search}`).toBe(
      "POST /api/v1/projects/banskabystrica/endpoints?dryRun=All",
    );
    await expect(checked.clone().json()).resolves.toMatchObject({
      draft: { kind: "Endpoint", name: "public-air" },
      spec: { slug: SLUG },
    });

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: en.endpoints.propose })).toBeEnabled(),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(proposals(fetchMock)).toHaveLength(1));
    const request = proposals(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/endpoints/public-air",
    );
    const body = (await request.clone().json()) as {
      spec: { slug: string; enabledRepresentations: string[] };
    };
    expect(body.spec.slug).toBe(SLUG);
    expect(body.spec.enabledRepresentations).toEqual(["ngsi-ld", "geojson", "csv"]);
  });

  it("names the audiences in words and still saves the contract's value", async () => {
    const fetchMock = renderEndpoints();
    await screen.findByText("public-air");
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.add }));
    const dialog = await screen.findByRole("dialog");

    const audience = dialog.querySelector("#root_audience") as HTMLSelectElement;
    const labels = [...audience.options].map((option) => option.textContent);
    expect(labels).toEqual(
      expect.arrayContaining([
        en.endpoints.audienceOption.public,
        en.endpoints.audienceOption.organization,
        en.endpoints.audienceOption["project-list"],
      ]),
    );
    expect(labels).not.toContain("project-list");

    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "air-open");
    await userEvent.selectOptions(audience, en.endpoints.audienceOption.public);
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: en.endpoints.propose })).toBeEnabled(),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(proposals(fetchMock)).toHaveLength(1));
    const body = (await proposals(fetchMock)[0].clone().json()) as {
      spec: { audience: string; enabledRepresentations: string[] };
    };
    expect(body.spec.audience).toBe("public");
    expect(body.spec.enabledRepresentations).toEqual(["ngsi-ld"]);
  });

  it("runs the check as a dry run of the envelope that names the held draft (PF-57, AG-61)", async () => {
    const fetchMock = renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(await rowMenuItem(row, en.endpoints.edit));

    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    const url = new URL(request.url);
    expect(url.pathname).toBe("/api/v1/projects/banskabystrica/endpoints");
    expect(url.searchParams.get("dryRun")).toBe("All");
    const body = (await request.clone().json()) as {
      kind: string;
      draft: { kind: string; name: string };
      spec: { slug: string };
    };
    expect(body.kind).toBe("Endpoint");
    expect(body.draft).toEqual({ kind: "Endpoint", name: "public-air" });
    expect(body.spec.slug).toBe(SLUG);
  });
});

/**
 * T-1433, UI-69, EP-07: what an endpoint answers is two clicks from the list, read only, with the
 * person's own session. The published schema decides which types and attributes are offered, so a
 * hidden attribute is absent from the grid as it is from every representation.
 */
describe("what an endpoint answers", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  async function openData(entities?: () => Response) {
    const fetchMock = renderEndpoints(undefined, entities);
    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.endpoints.data.action }));
    return { fetchMock, dialog: await screen.findByRole("dialog") };
  }

  it("opens the grid on the endpoint's first published type, through the endpoint itself", async () => {
    const { fetchMock, dialog } = await openData();

    expect(within(dialog).getByLabelText(en.endpoints.data.type)).toHaveValue("AirQualityObserved");
    expect(await within(dialog).findByText("12 GQ")).toBeInTheDocument();
    const reads = fetchMock.mock.calls
      .map((call) => callOf(call[0], call[1] as RequestInit | undefined).url)
      .filter((url) => url.pathname.includes("/ngsi-ld/v1/entities"));
    expect(reads).not.toHaveLength(0);
    expect(reads[0].pathname).toBe(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`);
    expect(reads[0].searchParams.get("type")).toBe("AirQualityObserved");
    // Read only: nothing was written to open it.
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("leaves a hidden attribute out of the columns", async () => {
    const hidden = {
      ...ENDPOINTS,
      items: [
        {
          ...ENDPOINTS.items[0],
          spec: { ...ENDPOINTS.items[0].spec, projection: { hiddenAttributes: ["sensorSerial"] } },
        },
      ],
    };
    const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const { url } = callOf(input, init);
      const json = (body: unknown) =>
        Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
        );
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 1 }] });
      if (url.pathname.endsWith("/schema/v1/json-schema")) return json(PUBLISHED);
      if (url.pathname.includes("/ngsi-ld/v1/entities")) return json(AIR_ROWS);
      if (url.pathname.endsWith("/endpoints")) return json(hidden);
      if (url.pathname.endsWith("/spaces")) return json(SPACES);
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

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.endpoints.data.action }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("12 GQ");
    // The attribute the endpoint hides has no column, and its value is nowhere on the page.
    expect(within(dialog).queryByRole("columnheader", { name: /sensorSerial/ })).toBeNull();
    expect(within(dialog).queryByText("SN-9")).toBeNull();
  });

  it("says why an endpoint that is not Live answers nothing, and offers no data view", async () => {
    const draft = {
      ...ENDPOINTS,
      items: [{ ...ENDPOINTS.items[0], status: { phase: "PendingApproval" } }],
    };
    const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const { url } = callOf(input, init);
      const json = (body: unknown) =>
        Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
        );
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/endpoints")) return json(draft);
      if (url.pathname.endsWith("/spaces")) return json(SPACES);
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

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    const button = within(row).getByRole("button", { name: en.endpoints.data.action });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", en.endpoints.data.notLive);
    // Nothing was read from the gateway for an endpoint that serves nothing.
    expect(
      fetchMock.mock.calls.some((call) =>
        callOf(call[0], call[1] as RequestInit | undefined).url.pathname.includes("/ngsi-ld/v1/"),
      ),
    ).toBe(false);
  });

  it("shows the gateway's own refusal when the person may not read the endpoint", async () => {
    const { dialog } = await openData(
      () =>
        new Response(
          JSON.stringify({ title: "Forbidden", detail: "no grant lets you read AirQualityObserved here" }),
          { status: 403, headers: { "Content-Type": "application/problem+json" } },
        ),
    );

    expect(
      await within(dialog).findByText(/no grant lets you read AirQualityObserved here/),
    ).toBeInTheDocument();
  });

  it("puts the same grid in the form, under what the endpoint publishes", async () => {
    renderEndpoints();
    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(await rowMenuItem(row, en.endpoints.edit));

    const dialog = await screen.findByRole("dialog");
    // Folded until a person asks for it: the form is long enough already.
    await userEvent.click(within(dialog).getByText(en.endpoints.data.title));
    expect(await within(dialog).findByText("12 GQ")).toBeInTheDocument();
  });
});

