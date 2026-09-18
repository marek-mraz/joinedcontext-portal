import userEvent from "@testing-library/user-event";
import { render, screen, waitFor, within } from "@testing-library/react";
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
  roles: ["portal-approver"],
};

function proposal(over: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
    summary: {
      key: "change.summary.update",
      params: { kind: "Endpoint", name: "air-quality", fields: 2 },
    },
    author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
    createdAt: "2026-03-03T12:00:00Z",
    status: {
      lane: "yellow",
      phase: "PendingApproval",
      plan: { update: 1 },
      mergeRequest: "https://gitea.example/city/city-config/pulls/7",
    },
    planFields: [{ path: "spec.audience", from: "public", to: "internal" }],
    ...over,
  };
}

const CHANGES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeList",
  items: [
    proposal(),
    proposal({
      metadata: { name: "chg-9f8e7d6c", namespace: "banskabystrica" },
      summary: {
        key: "change.summary.delete",
        params: { kind: "ContextSpace", name: "doprava", fields: 0 },
      },
      author: { name: "Peter Novák", email: "peter@banskabystrica.sk" },
      createdAt: "2026-03-04T12:00:00Z",
      status: { lane: "red", phase: "PendingApproval", plan: { delete: 1 } },
    }),
  ],
};

function renderApprovals(body: unknown = CHANGES) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const payload = url.includes("/auth/me") ? IDENTITY : body;
    return Promise.resolve(
      new Response(JSON.stringify(payload), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
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

describe("pending approvals view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/approvals");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("asks the changes endpoint of the project in the URL", async () => {
    const fetchMock = renderApprovals();
    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as RequestInfo | URL;
        return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      });
      expect(urls.some((url) => url.endsWith("/api/v1/projects/banskabystrica/changes"))).toBe(
        true,
      );
    });
  });

  it("summarises each proposal in words, not in field paths", async () => {
    renderApprovals();
    expect(
      await screen.findByRole("link", { name: 'Update Endpoint "air-quality" (2 fields changed)' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: 'Delete ContextSpace "doprava"' }),
    ).toBeInTheDocument();
  });

  it("shows the risk lane of every proposal as a chip", async () => {
    renderApprovals();
    await screen.findByText(en.lane.yellow);
    expect(screen.getByText(en.lane.yellow)).toHaveAttribute("title", en.lane.yellowHelp);
    expect(screen.getByText(en.lane.red)).toBeInTheDocument();
  });

  it("shows the phase of every proposal as a chip", async () => {
    renderApprovals({
      ...CHANGES,
      items: [CHANGES.items[0], { ...CHANGES.items[1], status: { ...CHANGES.items[1].status, phase: "Deploying" } }],
    });

    await screen.findByText("chg-9f8e7d6c");
    const table = screen.getByRole("table");
    expect(within(table).getByText(en.phase.pendingApproval)).toBeInTheDocument();
    expect(within(table).getByText(en.phase.deploying)).toBeInTheDocument();
  });

  it("narrows the list to the caller's own proposals", async () => {
    const mineToo = proposal({
      metadata: { name: "chg-00000001", namespace: "banskabystrica" },
      author: { name: "Jana Kováčová", email: "Jana.Kovacova@banskabystrica.sk" },
    });
    renderApprovals({ ...CHANGES, items: [...CHANGES.items, mineToo] });

    expect(await screen.findByText("chg-9f8e7d6c")).toBeInTheDocument();
    await userEvent.click(screen.getByLabelText(en.approvals.filterMine));
    expect(screen.getByText("chg-00000001")).toBeInTheDocument();
    expect(screen.queryByText("chg-9f8e7d6c")).toBeNull();
    expect(screen.queryByText("chg-1a2b3c4d")).toBeNull();
  });

  it("narrows the list to one phase, and says so when nothing is left", async () => {
    renderApprovals({
      ...CHANGES,
      items: [CHANGES.items[0], { ...CHANGES.items[1], status: { ...CHANGES.items[1].status, phase: "Deploying" } }],
    });

    await userEvent.selectOptions(await screen.findByLabelText(en.approvals.filterPhase), "Deploying");
    expect(screen.getByText("chg-9f8e7d6c")).toBeInTheDocument();
    expect(screen.queryByText("chg-1a2b3c4d")).toBeNull();

    await userEvent.click(screen.getByLabelText(en.approvals.filterMine));
    expect(screen.getByText(en.approvals.noneMatch)).toBeInTheDocument();
    // The filters stay, so the person can widen the list again.
    expect(screen.getByLabelText(en.approvals.filterMine)).toBeChecked();
  });

  it("attributes each proposal to the person who wrote it", async () => {
    renderApprovals();
    const row = (await screen.findByText("chg-1a2b3c4d")).closest("tr") as HTMLElement;
    expect(within(row).getByText("Marek Mráz")).toBeInTheDocument();
    expect(within(row).getByText(/Mar 3, 2026/)).toBeInTheDocument();
    expect(screen.getByText("Peter Novák")).toBeInTheDocument();
  });

  it("links a proposal to its own detail route", async () => {
    renderApprovals();
    const link = await screen.findByRole("link", {
      name: 'Update Endpoint "air-quality" (2 fields changed)',
    });
    expect(link).toHaveAttribute("href", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  it("says the queue is empty rather than showing a bare table", async () => {
    renderApprovals({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [] });
    expect(await screen.findByText(en.approvals.empty)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });
});

describe("a bundle in the queue (T-0861)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/approvals");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says how many files a change carries when it carries more than one", async () => {
    renderApprovals({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ChangeList",
      items: [
        proposal({ fileCount: 3 }),
        proposal({
          metadata: { name: "chg-9f8e7d6c", namespace: "banskabystrica" },
          fileCount: 1,
        }),
      ],
    });

    const bundle = (await screen.findByText("chg-1a2b3c4d")).closest("tr") as HTMLElement;
    expect(within(bundle).getByText("3 files")).toBeInTheDocument();
    const single = screen.getByText("chg-9f8e7d6c").closest("tr") as HTMLElement;
    expect(within(single).queryByText(/file/)).not.toBeInTheDocument();
  });
});
