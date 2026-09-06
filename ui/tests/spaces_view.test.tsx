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
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: {
        name: "ovzdusie",
        namespace: "banskabystrica",
        title: { sk: "Ovzdušie", en: "Air quality" },
      },
      spec: { dataModelRef: "AirQualityObserved" },
      status: {
        phase: "Live",
        observedRevision: "9c1f0ab",
        sourceUrl:
          "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/spaces/ovzdusie/space.yaml",
      },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "doprava", namespace: "banskabystrica" },
      spec: { isSandbox: true, ttlDays: 7 },
      status: { phase: "Deploying" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};

function project(quota?: number) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Project",
    metadata: { name: "banskabystrica", namespace: "banskabystrica" },
    spec: {
      organizationRef: "banskabystrica.sk",
      ...(quota === undefined ? {} : { quotas: { contextSpaces: quota } }),
    },
    status: { phase: "Live" },
  };
}

function renderSpaces(options: { quota?: number } = { quota: 3 }) {
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
    if (request.method === "POST") {
      return json(CHANGE, 202);
    }
    if (path.endsWith("/projects/banskabystrica")) {
      return json(project(options.quota));
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
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

function posts(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method === "POST");
}

describe("context spaces view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every space with its title, data model and lifecycle state", async () => {
    renderSpaces();

    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    expect(within(row).getByText("Air quality")).toBeInTheDocument();
    expect(within(row).getByText("AirQualityObserved")).toBeInTheDocument();
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();

    const sandbox = (await screen.findByText(/sandbox, 7 days/)).closest("tr") as HTMLElement;
    expect(within(sandbox).getByText("doprava")).toBeInTheDocument();
    expect(within(sandbox).getByText(en.phase.deploying)).toBeInTheDocument();
  });

  it("links each mirrored space to its manifest in the forge", async () => {
    renderSpaces();
    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    expect(within(row).getByRole("link", { name: en.spaces.field.source })).toHaveAttribute(
      "href",
      SPACES.items[0].status.sourceUrl,
    );
  });

  it("counts the used quota against the project limit", async () => {
    renderSpaces({ quota: 3 });

    const bar = await screen.findByRole("progressbar", { name: en.quota.contextSpaces });
    expect(bar).toHaveAttribute("aria-valuenow", "2");
    expect(bar).toHaveAttribute("aria-valuemax", "3");
    expect(screen.getByText("2 / 3")).toBeInTheDocument();
  });

  it("blocks a new space once the quota is used up", async () => {
    renderSpaces({ quota: 2 });

    await waitFor(() =>
      expect(screen.getByRole("button", { name: en.spaces.add })).toBeDisabled(),
    );
    expect(screen.getByText(/quota of 2 Context Spaces is used up/)).toBeInTheDocument();
  });

  it("proposes a change instead of writing the space directly", async () => {
    const fetchMock = renderSpaces();

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mobilita");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    const request = posts(fetchMock)[0];
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/spaces");
    await expect(request.clone().json()).resolves.toMatchObject({
      kind: "ContextSpace",
      metadata: { name: "mobilita", namespace: "banskabystrica" },
    });

    expect(await screen.findByText("chg-1a2b3c4d")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: en.changes.review }),
    ).toHaveAttribute("href", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  it("refuses a name that is not a lowercase slug (PF-09)", async () => {
    const fetchMock = renderSpaces();

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "Mobilita Mesta");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(en.form.pattern);
    expect(posts(fetchMock)).toHaveLength(0);
  });
});
