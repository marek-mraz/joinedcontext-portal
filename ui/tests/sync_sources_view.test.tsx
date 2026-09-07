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
  roles: ["portal-approver"],
};

const SOURCES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "SyncSource",
      metadata: { name: "regional-datamodels", namespace: "banskabystrica" },
      spec: {
        source: {
          git: {
            url: "https://git.region.sk/udp/datamodels.git",
            ref: "main",
            path: "models/transport",
          },
        },
        schedule: { interval: "30m" },
        mode: "mirror",
      },
    },
  ],
};

const PENDING = {
  project: "banskabystrica",
  name: "regional-datamodels",
  phase: "PendingApproval",
  observedRevision: null,
  lastRunAt: 1757000000,
  mergeRequest: "https://forge.example/pulls/7",
  lastError: null,
  paused: false,
  durable: true,
};

/** Renders the sync view with a status the test chooses, and records what was called. */
function renderSyncSources(status: Record<string, unknown> = PENDING) {
  const calls: { method: string; path: string; body?: string }[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const body = request.method === "GET" ? undefined : await request.clone().text();
    calls.push({ method: request.method, path, body });
    const json = (payload: unknown, code = 200) =>
      new Response(JSON.stringify(payload), {
        status: code,
        headers: { "Content-Type": "application/json" },
      });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/branding")) {
      return json({
        instanceName: "joinedcontext",
        languages: { default: "en", offered: ["en"] },
      });
    }
    if (path.endsWith("/status")) {
      return json(status);
    }
    if (path.endsWith("/pause")) {
      return json({ ...status, paused: !status.paused });
    }
    if (path.endsWith("/detach")) {
      return json({ mergeRequest: "https://forge.example/pulls/9", number: 9 }, 202);
    }
    if (path.endsWith("/syncsources/regional-datamodels/sync")) {
      return json({
        proposed: [],
        unchanged: 1,
        flags: [],
        status,
      });
    }
    if (path.endsWith("/syncsources")) {
      return json(SOURCES);
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
  return calls;
}

describe("sync sources view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/syncsources");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says where a source reads from, how often, and where it stands (MF-27, MF-30)", async () => {
    renderSyncSources();

    const card = (await screen.findByText("regional-datamodels")).closest(
      "article",
    ) as HTMLElement;
    expect(
      within(card).getByText("https://git.region.sk/udp/datamodels.git#main (models/transport)"),
    ).toBeInTheDocument();
    expect(within(card).getByText("Every 30m")).toBeInTheDocument();
    expect(await within(card).findByText(en.phase.pendingApproval)).toBeInTheDocument();
    expect(await within(card).findByRole("link", { name: en.syncSources.review })).toHaveAttribute(
      "href",
      "https://forge.example/pulls/7",
    );
  });

  it("says why a run failed instead of leaving it to the log (MF-30)", async () => {
    renderSyncSources({
      ...PENDING,
      phase: "Error",
      mergeRequest: null,
      lastError: "sync origin unavailable: the origin did not answer in time",
    });

    const card = (await screen.findByText("regional-datamodels")).closest(
      "article",
    ) as HTMLElement;
    expect(await within(card).findByText(en.phase.error)).toBeInTheDocument();
    expect(
      await within(card).findByText("sync origin unavailable: the origin did not answer in time"),
    ).toBeInTheDocument();
  });

  it("runs one source when Sync now is pressed, and says a quiet run changed nothing", async () => {
    const calls = renderSyncSources();
    await screen.findByText("regional-datamodels");

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.syncNow }));

    await waitFor(() => {
      expect(
        calls.some(
          (call) =>
            call.method === "POST" &&
            call.path ===
              "/api/v1/projects/banskabystrica/syncsources/regional-datamodels/sync",
        ),
      ).toBe(true);
    });
    expect(await screen.findByText(en.syncSources.nothingToDo)).toBeInTheDocument();
  });

  it("pauses and resumes without touching the repository (MF-30)", async () => {
    const calls = renderSyncSources();
    await screen.findByText("regional-datamodels");

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.pause }));

    await waitFor(() => {
      const pause = calls.find((call) => call.path.endsWith("/pause"));
      expect(pause?.body).toBe(JSON.stringify({ paused: true }));
    });
    expect(
      calls.some((call) => call.method !== "GET" && call.path.includes("/changes")),
    ).toBe(false);
  });

  it("a paused source offers Resume and no run at all", async () => {
    renderSyncSources({ ...PENDING, phase: "Paused", paused: true, mergeRequest: null });
    await screen.findByText("regional-datamodels");

    expect(await screen.findByRole("button", { name: en.syncSources.resume })).toBeEnabled();
    expect(screen.getByRole("button", { name: en.syncSources.syncNow })).toBeDisabled();
  });

  it("asks before detaching, and answers with the merge request that removes it", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const calls = renderSyncSources();
    await screen.findByText("regional-datamodels");

    await userEvent.click(screen.getByRole("button", { name: en.syncSources.detach }));
    expect(confirm).toHaveBeenCalled();
    expect(calls.some((call) => call.path.endsWith("/detach"))).toBe(false);

    confirm.mockReturnValue(true);
    await userEvent.click(screen.getByRole("button", { name: en.syncSources.detach }));

    await waitFor(() => {
      expect(calls.some((call) => call.path.endsWith("/detach"))).toBe(true);
    });
    const notice = await screen.findByRole("status");
    expect(notice).toHaveTextContent(en.syncSources.detached);
    expect(within(notice).getByRole("link", { name: en.syncSources.review })).toHaveAttribute(
      "href",
      "https://forge.example/pulls/9",
    );
  });

  it("warns when a restart would forget where each source stood", async () => {
    renderSyncSources({ ...PENDING, durable: false });
    expect(await screen.findByText(en.syncSources.notDurable)).toBeInTheDocument();
  });
});
