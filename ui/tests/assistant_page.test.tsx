import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { onOpenRequest } from "../src/assistant/state";

const PROJECT = "banskabystrica";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

const CONV_RUN = {
  id: "run-conv-1",
  project: PROJECT,
  appName: "",
  endpointName: "",
  endpointSlug: "",
  profile: "app-builder",
  appClass: "static",
  visibility: "private",
  kind: "conversation",
  unattended: false,
  continues: null,
  prompt: "Find all air quality sensors in town",
  status: "cancelled",
  steps: 2,
  tokensUsed: 500,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T09:00:00Z",
};

const WORK_RUN = {
  id: "run-work-1",
  project: PROJECT,
  appName: "city-bikes-app",
  endpointName: "bikes-endpoint",
  endpointSlug: "slug123",
  profile: "app-builder",
  appClass: "static",
  visibility: "project",
  kind: "application",
  unattended: true,
  continues: null,
  prompt: "Create city bikes overview",
  status: "awaiting_approval",
  firstFrameMs: 1200,
  firstVersionMs: 2500,
  steps: 5,
  tokensUsed: 2500,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T10:00:00Z",
};

const CONTINUED_RUN = {
  id: "run-conv-2",
  project: PROJECT,
  appName: "",
  endpointName: "",
  endpointSlug: "",
  profile: "app-builder",
  appClass: "static",
  visibility: "private",
  kind: "conversation",
  unattended: false,
  continues: "run-conv-1",
  prompt: "Continue sensor analysis",
  status: "interviewing",
  steps: 1,
  tokensUsed: 200,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T11:00:00Z",
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "bikes-endpoint" },
      spec: { slug: "slug123" },
    },
  ],
};

class StubEventSource {
  addEventListener(): void {}
  removeEventListener(): void {}
  close(): void {}
}

let requests: Request[] = [];
const fetchCalls = () => requests;

function renderAssistantPage(
  runs: unknown[] = [CONV_RUN, WORK_RUN, CONTINUED_RUN],
  postRunHandler?: (req: Request) => Promise<Response>,
) {
  requests = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init);
    requests.push(request);
    const url = new URL(request.url, "http://localhost");
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: {
            "Content-Type": status >= 400 ? "application/problem+json" : "application/json",
          },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/projects")) {
      return json([PROJECT]);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/assistant/conversations") && request.method === "POST") {
      return json(
        {
          id: "new-conv-run-id",
          project: PROJECT,
          appName: "",
          endpointName: "",
          endpointSlug: "",
          appClass: "static",
          visibility: "private",
          kind: "conversation",
          unattended: false,
          prompt: en.assistantPage.continueMessage,
          status: "interviewing",
          steps: 0,
          tokensUsed: 0,
          createdBy: "jana.kovacova",
          createdAt: "2026-09-14T12:00:00Z",
        },
        202,
      );
    }
    if (path.endsWith("/agent-runs")) {
      if (request.method === "POST") {
        if (postRunHandler) {
          return postRunHandler(request);
        }
        return json(
          {
            id: "new-work-run-id",
            project: PROJECT,
            appName: "new-work-app",
            endpointName: "bikes-endpoint",
            endpointSlug: "slug123",
            appClass: "static",
            visibility: "project",
            kind: "dashboard",
            unattended: true,
            prompt: "Create bike dashboard",
            status: "queued",
            steps: 0,
            tokensUsed: 0,
            createdBy: "jana.kovacova",
            createdAt: "2026-09-14T12:00:00Z",
          },
          202,
        );
      }
      return json({ items: runs });
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });

  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("Assistant page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists a conversation and a work run with their kind labels", async () => {
    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    expect(await screen.findByText("Find all air quality sensors in town")).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getAllByText(i18n.t("assistantPage.kinds.conversation"))).toHaveLength(2);

    expect(within(table).getByText("City bikes app")).toBeInTheDocument();
    expect(within(table).queryByText("city-bikes-app")).not.toBeInTheDocument();
    // One header row whose five columns line up with every body row's cells.
    const rows = within(table).getAllByRole("row");
    expect(within(rows[0]).getAllByRole("columnheader")).toHaveLength(5);
    for (const row of rows.slice(1)) {
      expect(within(row).getAllByRole("cell")).toHaveLength(5);
    }
    expect(within(table).getByText(i18n.t("assistantPage.kinds.application"))).toBeInTheDocument();

    expect(within(table).getByText("Continue sensor analysis")).toBeInTheDocument();
    expect(within(table).getByText(i18n.t("assistantPage.continues"))).toBeInTheDocument();
  });

  it("puts kind= in the request when filtering by kind", async () => {
    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const kindSelect = screen.getByLabelText(i18n.t("assistantPage.filters.kind"));
    const user = userEvent.setup();
    await user.selectOptions(kindSelect, "conversation");

    await waitFor(() => {
      expect(
        fetchCalls().some((req) => {
          const url = new URL(req.url, "http://localhost");
          return url.pathname.endsWith("/agent-runs") && url.searchParams.get("kind") === "conversation";
        }),
      ).toBe(true);
    });
  });

  it("remembers the run in sessionStorage and fires open request when Open is clicked", async () => {
    let opened = false;
    const unsub = onOpenRequest(() => {
      opened = true;
    });

    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const promptEl = await screen.findByText("Find all air quality sensors in town");
    const row = (promptEl.closest("tr") ?? promptEl.closest("li")) as HTMLElement;
    const openButton = within(row).getByRole("button", { name: i18n.t("assistantPage.open") });

    const user = userEvent.setup();
    await user.click(openButton);

    expect(opened).toBe(true);
    const stored = JSON.parse(window.sessionStorage.getItem("jc.assistant.run") ?? "{}") as {
      project: string;
      runId: string;
    };
    expect(stored.runId).toBe(CONV_RUN.id);
    expect(stored.project).toBe(PROJECT);

    unsub();
  });

  it("posts continues when Continue is clicked on an ended conversation", async () => {
    let opened = false;
    const unsub = onOpenRequest(() => {
      opened = true;
    });

    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const promptEl = await screen.findByText("Find all air quality sensors in town");
    const row = (promptEl.closest("tr") ?? promptEl.closest("li")) as HTMLElement;
    const continueButton = within(row).getByRole("button", {
      name: i18n.t("assistantPage.continue"),
    });

    const user = userEvent.setup();
    await user.click(continueButton);

    await waitFor(() => {
      const postReq = fetchCalls().find(
        (req) => req.method === "POST" && req.url.includes("/assistant/conversations"),
      );
      expect(postReq).toBeDefined();
    });

    const postReq = fetchCalls().find(
      (req) => req.method === "POST" && req.url.includes("/assistant/conversations"),
    )!;
    const body = (await postReq.json()) as { continues: string; message: string };
    expect(body.continues).toBe(CONV_RUN.id);
    expect(body.message).toBe(i18n.t("assistantPage.continueMessage"));

    expect(opened).toBe(true);
    const stored = JSON.parse(window.sessionStorage.getItem("jc.assistant.run") ?? "{}") as {
      project: string;
      runId: string;
    };
    expect(stored.runId).toBe("new-conv-run-id");
    expect(stored.project).toBe(PROJECT);

    unsub();
  });

  it("posts kind and unattended: true for new work", async () => {
    const user = userEvent.setup();
    renderAssistantPage();
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const kindSelect = screen.getByLabelText(i18n.t("assistantPage.newWork.kind"));
    await user.selectOptions(kindSelect, "dashboard");

    const nameInput = screen.getByLabelText(i18n.t("assistantPage.newWork.name"));
    await user.type(nameInput, "city-dashboard");

    const endpointSelect = screen.getByLabelText(i18n.t("assistantPage.newWork.endpoint"));
    await user.selectOptions(endpointSelect, "bikes-endpoint");

    const promptInput = screen.getByLabelText(i18n.t("assistantPage.newWork.prompt"));
    await user.type(promptInput, "Create bike dashboard");

    const startButton = screen.getByRole("button", {
      name: i18n.t("assistantPage.newWork.start"),
    });
    await user.click(startButton);

    await waitFor(() => {
      const postReq = fetchCalls().find(
        (req) => req.method === "POST" && req.url.endsWith("/agent-runs"),
      );
      expect(postReq).toBeDefined();
    });

    const postReq = fetchCalls().find(
      (req) => req.method === "POST" && req.url.endsWith("/agent-runs"),
    )!;
    const body = (await postReq.json()) as {
      kind: string;
      unattended: boolean;
      appName: string;
      endpointName: string;
      prompt: string;
      appClass: string;
      visibility: string;
      dataNeeds: unknown[];
    };
    expect(body.kind).toBe("dashboard");
    expect(body.unattended).toBe(true);
    expect(body.appName).toBe("city-dashboard");
    expect(body.endpointName).toBe("bikes-endpoint");
    expect(body.prompt).toBe("Create bike dashboard");
  });

  it("shows the problem detail when new work is refused with 409", async () => {
    const conflictResponse = {
      type: "https://joinedcontext.com/errors/conflict",
      title: "Conflict",
      status: 409,
      detail: "application 'city-dashboard' already has a live run: run-work-1",
    };
    const user2 = userEvent.setup();
    renderAssistantPage(
      [CONV_RUN, WORK_RUN],
      async () =>
        new Response(JSON.stringify(conflictResponse), {
          status: 409,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    await screen.findByRole("heading", { level: 1, name: i18n.t("assistantPage.title") });

    const kindSelect2 = screen.getByLabelText(i18n.t("assistantPage.newWork.kind"));
    await user2.selectOptions(kindSelect2, "dashboard");

    const nameInput2 = screen.getByLabelText(i18n.t("assistantPage.newWork.name"));
    await user2.type(nameInput2, "city-dashboard");

    const endpointSelect2 = screen.getByLabelText(i18n.t("assistantPage.newWork.endpoint"));
    await user2.selectOptions(endpointSelect2, "bikes-endpoint");

    const promptInput2 = screen.getByLabelText(i18n.t("assistantPage.newWork.prompt"));
    await user2.type(promptInput2, "Create bike dashboard");

    const startButton2 = screen.getByRole("button", {
      name: i18n.t("assistantPage.newWork.start"),
    });
    await user2.click(startButton2);

    expect(
      await screen.findByText("application 'city-dashboard' already has a live run: run-work-1"),
    ).toBeInTheDocument();
  });
});
