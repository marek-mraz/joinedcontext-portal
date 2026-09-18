import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { answeringChecks, checksSoFar } from "./checks";

const PROJECT = "helsinki";

const ACCESS = {
  items: [
    {
      name: "app-builder",
      title: "Application builder",
      role: "builder",
      access: { operations: ["jc_catalog_search", "jc_endpoint_propose"] },
      egressHosts: ["registry.npmjs.org"],
      operations: [
        { name: "jc_catalog_search", readOnly: true, profile: true, person: true, reason: null },
        { name: "jc_endpoint_propose", readOnly: false, profile: true, person: false, reason: "no role grants propose on Endpoint" },
        { name: "jc_space_complete", readOnly: false, profile: false, person: true, reason: "the agent profile does not grant jc_space_complete (AG-70)" },
      ],
    },
  ],
};

const PROFILE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "AgentProfile",
  metadata: { name: "app-builder", namespace: "org" },
  spec: { role: "builder", access: ACCESS.items[0].access },
  status: { phase: "Live" },
};

let puts: unknown[] = [];

function renderPage() {
  puts = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const path = new URL(request.url, "http://localhost").pathname;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (path.endsWith("/auth/me")) return json({ subject: "s", username: "reader", roles: [] });
      if (path.endsWith("/projects")) return json([PROJECT]);
      if (path.endsWith("/assistant/access")) return json(ACCESS);
      if (path.endsWith("/projects/org/agentprofiles/app-builder") && request.method === "GET") return json(PROFILE);
      if (path.endsWith("/projects/org/agentprofiles/app-builder") && request.method === "PUT") {
        puts.push(await request.json());
        return json(
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "pr-7" },
            status: { lane: "yellow", phase: "Pending" },
          },
          202,
        );
      }
      return json({ items: [] });
    }),
  );
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  vi.stubGlobal("EventSource", class { addEventListener() {} removeEventListener() {} close() {} });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("Agent access on the Assistant page (UI-56)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows both halves per operation and the reason one refuses", async () => {
    renderPage();
    const table = await screen.findByRole("table", { name: i18n.t("assistantPage.access.caption", { name: "app-builder" }) });
    const rows = within(table).getAllByRole("row");
    const cells = (name: string) =>
      within(rows.find((row) => within(row).queryByText(name)) as HTMLElement)
        .getAllByRole("cell")
        .map((cell) => cell.textContent);
    expect(cells("jc_catalog_search")).toEqual(["jc_catalog_search", "yes", "yes", ""]);
    expect(cells("jc_endpoint_propose")).toEqual(["jc_endpoint_propose", "yes", "no", "no role grants propose on Endpoint"]);
    expect(cells("jc_space_complete")[1]).toBe("no");
    expect(screen.getByText(i18n.t("assistantPage.access.available", { count: 1, total: 3 }))).toBeInTheDocument();
    expect(screen.getByText("registry.npmjs.org")).toBeInTheDocument();
  });

  it("proposes an edited access block as a change on the profile, without its status", async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: i18n.t("assistantPage.access.edit") }));
    const block = screen.getByLabelText(i18n.t("assistantPage.access.block"));
    await user.clear(block);
    await user.type(block, "operations: [[jc_catalog_search]");
    await user.click(screen.getByRole("button", { name: i18n.t("assistantPage.access.propose") }));

    await waitFor(() => expect(puts).toHaveLength(1));
    const sent = puts[0] as { spec: { access: unknown; role: string }; status?: unknown };
    expect(sent.spec.access).toEqual({ operations: ["jc_catalog_search"] });
    expect(sent.spec.role).toBe("builder");
    expect(sent.status).toBeUndefined();
    expect(await screen.findByText("pr-7")).toBeInTheDocument();
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("PUT /api/v1/projects/org/agentprofiles/app-builder"))).toBe(true);
  });
});
