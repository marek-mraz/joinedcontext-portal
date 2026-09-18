import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { answeringChecks, checksSoFar } from "./checks";

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
      // The reference as "Complete this space" writes it: `{ kind, name }` (MF-07).
      spec: { isSandbox: true, ttlDays: 7, dataModelRef: { kind: "DataModel", name: "citybikes" } },
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

/** The project as `GET /api/v1/projects/{project}` answers it: the manifest and its usage. */
function project(quota?: number) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Project",
    metadata: { name: "banskabystrica", namespace: "org" },
    spec: { organizationRef: "banskabystrica.sk" },
    status: {
      usage: {
        apps: { used: 0 },
        contextSpaces: { used: 2, ...(quota === undefined ? {} : { limit: quota }) },
        publicEndpoints: { used: 1, limit: 4 },
        residentPipelines: { used: 0, limit: 3 },
      },
    },
  };
}

function renderSpaces(options: { quota?: number; refusal?: string; listFails?: string } = { quota: 3 }) {
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
      return options.refusal
        ? json({ status: 403, title: "Forbidden", detail: options.refusal }, 403)
        : json(CHANGE, 202);
    }
    if (path === "/api/v1/projects/banskabystrica") {
      return json(project(options.quota));
    }
    if (path.includes("/projects/banskabystrica/projects")) {
      return json({ status: 404, title: "Resource Not Found" }, 404);
    }
    if (path.endsWith("/spaces")) {
      return options.listFails
        ? json({ status: 503, title: "Service Unavailable", detail: options.listFails }, 503)
        : json(SPACES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));

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
    expect(within(sandbox).getByText("citybikes")).toBeInTheDocument();
    expect(within(sandbox).getByText(en.phase.deploying)).toBeInTheDocument();
  });

  /// T-0961, UI-01: a list that cannot be read says so where the list would be, in the API's
  /// own words, with the way back. It does not leave a person watching an empty table.
  it("puts the API's own reason where the table would be, with a retry", async () => {
    const fetchMock = renderSpaces({ listFails: "the mirror is still loading" });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("the mirror is still loading");
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByText("ovzdusie")).toBeNull();

    const before = fetchMock.mock.calls.length;
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => {
      expect(fetchMock.mock.calls.length).toBeGreaterThan(before);
    });
  });

  it("links each mirrored space to its manifest in the forge", async () => {
    renderSpaces();
    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    expect(within(row).getByRole("link", { name: en.spaces.field.source })).toHaveAttribute(
      "href",
      SPACES.items[0].status.sourceUrl,
    );
  });

  it("shows every quota of the project with what it holds of it", async () => {
    renderSpaces({ quota: 3 });

    const bar = await screen.findByRole("progressbar", { name: en.quota.dimension.contextSpaces });
    expect(bar).toHaveAttribute("aria-valuenow", "2");
    expect(bar).toHaveAttribute("aria-valuemax", "3");
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    // Every dimension, not only the one this page creates; a dimension without a limit says so.
    expect(screen.getByText("1 of 4")).toBeInTheDocument();
    expect(screen.getByText(en.quota.dimension.residentPipelines)).toBeInTheDocument();
    expect(screen.getByText("0, no limit")).toBeInTheDocument();
  });

  it("puts the quota after the list, empty or not (T-1396)", async () => {
    renderSpaces({ quota: 3 });

    const bar = await screen.findByRole("progressbar", { name: en.quota.dimension.contextSpaces });
    const table = screen.getByRole("table");
    // DOCUMENT_POSITION_FOLLOWING: the bar comes after the table in reading order.
    expect(table.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("reads the numbers from the project route, which knows the quota in force", async () => {
    const fetchMock = renderSpaces({ quota: 3 });
    await screen.findByRole("progressbar", { name: en.quota.dimension.contextSpaces });
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).toContain("/api/v1/projects/banskabystrica");
    // The page no longer reads the Project manifest itself, so it cannot miss the
    // organization's default (PF-73).
    expect(paths).not.toContain("/api/v1/projects/org/projects");
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
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("POST /api/v1/projects/banskabystrica/spaces"))).toBe(true);
  });

  it("shows the name to use when another project already holds the one typed (PF-76)", async () => {
    renderSpaces({
      quota: 3,
      refusal:
        "context space name 'mhd' is taken by project doprava: a space name is unique in the " +
        "organization (PF-44); propose 'banskabystrica-mhd' instead",
    });

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "mhd");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("taken by project doprava");
    expect(alert).toHaveTextContent("banskabystrica-mhd");
  });

  it("refuses a name that is not a lowercase slug (PF-09)", async () => {
    const fetchMock = renderSpaces();

    await userEvent.click(await screen.findByRole("button", { name: en.spaces.add }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "Mobilita Mesta");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.propose }));

    // The message says what a name may be, rather than naming the pattern it broke (T-0960).
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(en.form.dns1123);
    expect(posts(fetchMock)).toHaveLength(0);
  });
});

/**
 * T-1030, UI-44 and PF-61: a viewer holds `read` on `ContextSpace` and no `propose`. Looking
 * inside a space is a read, so the row's link is theirs to follow; only the controls that
 * change something are kept from them. The roles unit of the recording shows exactly this.
 */
describe("a viewer on the spaces list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("follows the link into a space, and sees the controls that change one disabled with the reason (UI-44)", async () => {
    const viewer = {
      subject: "b7c1e0f4",
      username: "demo.viewer",
      name: "Demo Viewer",
      email: "demo.viewer@banskabystrica.sk",
      roles: ["portal-viewer"],
    };
    const readOnly = {
      project: "banskabystrica",
      bootstrap: false,
      grants: [{ role: "viewer", binding: "viewers", rule: { kinds: ["ContextSpace"], verbs: ["read"] } }],
    };
    const fetchMock = vi.fn(async (input: Request | string) => {
      const request = typeof input === "string" ? new Request(input) : input;
      const path = new URL(request.url, "http://localhost").pathname;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "content-type": "application/json" },
        });
      if (path.endsWith("/auth/me")) return json(viewer);
      if (path.endsWith("/permissions/me")) return json(readOnly);
      if (path.endsWith("/spaces")) return json(SPACES);
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const row = (await screen.findByText("ovzdusie")).closest("tr") as HTMLElement;
    const open = within(row).getByRole("link", { name: en.spaces.inside.open });
    expect(open).toBeInTheDocument();
    expect(open).toHaveAttribute("href", "/projects/banskabystrica/spaces/ovzdusie");
    // Nothing disables a link, so a viewer who may read is never stopped from looking.
    expect(open).not.toHaveAttribute("aria-disabled", "true");
    // What changes a space stays on the page, disabled, and says why (T-1383).
    // The guard remounts the control once the document arrives: queried after the wait.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: en.spaces.add })).toBeDisabled();
    });
    expect(screen.getByRole("button", { name: en.spaces.add }).parentElement).toHaveAttribute(
      "title",
      "Disabled: your role does not permit 'propose' on 'ContextSpace' in this project",
    );
    expect(within(row).getByRole("button", { name: /Delete/ })).toBeDisabled();
  });
});
