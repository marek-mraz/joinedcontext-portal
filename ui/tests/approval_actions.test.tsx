import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const APPROVER = {
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

const DEPLOYING = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-1a2b3c4d", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "Deploying", plan: { update: 1 } },
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function renderDetail(
  options: { identity?: unknown; change?: Record<string, unknown>; permissions?: unknown } = {},
) {
  const change = options.change ?? proposal();
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    if (request.url.includes("/auth/me")) {
      return Promise.resolve(json(options.identity ?? APPROVER));
    }
    // Roles as code (T-0526): the approver's grant comes from the bindings, not the token.
    if (request.url.includes("/permissions/me") && options.permissions) {
      return Promise.resolve(json(options.permissions));
    }
    if (request.url.includes("/permissions/me")) {
      const identity = (options.identity ?? APPROVER) as { roles?: string[] };
      const approver = identity.roles?.includes("portal-approver") ?? false;
      return Promise.resolve(
        json({ project: "banskabystrica", bootstrap: approver, grants: [] }),
      );
    }
    if (request.method === "POST") {
      return Promise.resolve(json(DEPLOYING, 202));
    }
    return Promise.resolve(json(change));
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

describe("approval actions", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-value";
    window.history.pushState({}, "", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("posts the approval to the change of the route, with the CSRF token", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.approve }));

    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    const request = posts(fetchMock)[0];
    expect(request.url).toContain(
      "/api/v1/projects/banskabystrica/changes/chg-1a2b3c4d/approve",
    );
    expect(request.headers.get("x-csrf-token")).toBe("csrf-token-value");
  });

  it("moves the status chip to Deploying once the merge is accepted", async () => {
    renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.approve }));

    expect(await screen.findByText(en.phase.deploying)).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: en.approvals.approve })).toBeDisabled(),
    );
  });

  it("asks why before it rejects, and sends the reason to the reject endpoint", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.reject }));

    const dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    expect(posts(fetchMock)).toHaveLength(0);
    await userEvent.type(within(dialog).getByLabelText(en.approvals.rejectReason), "  The slug is wrong  ");
    await userEvent.click(within(dialog).getByRole("button", { name: en.approvals.rejectConfirm }));

    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(posts(fetchMock)[0].url).toContain(
      "/api/v1/projects/banskabystrica/changes/chg-1a2b3c4d/reject",
    );
    await expect(posts(fetchMock)[0].clone().json()).resolves.toEqual({ reason: "The slug is wrong" });
  });

  it("rejects with no reason when none is given, and Cancel rejects nothing", async () => {
    const fetchMock = renderDetail();
    await userEvent.click(await screen.findByRole("button", { name: en.approvals.reject }));
    let dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    await userEvent.click(within(dialog).getAllByRole("button", { name: en.approvals.rejectCancel })[0]);
    expect(posts(fetchMock)).toHaveLength(0);

    await userEvent.click(screen.getByRole("button", { name: en.approvals.reject }));
    dialog = await screen.findByRole("dialog", { name: en.approvals.rejectTitle });
    await userEvent.click(within(dialog).getByRole("button", { name: en.approvals.rejectConfirm }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(await posts(fetchMock)[0].clone().text()).toBe("");
  });

  it("disables both actions without the approver role, and says why", async () => {
    const fetchMock = renderDetail({
      identity: { ...APPROVER, roles: ["portal-viewer"] },
    });

    expect(await screen.findByRole("button", { name: en.approvals.approve })).toBeDisabled();
    expect(screen.getByRole("button", { name: en.approvals.reject })).toBeDisabled();
    expect(screen.getByText(en.approvals.needsRole)).toBeInTheDocument();
    // On the button itself too: by pointer the wrapper's tooltip, by keyboard its description.
    const approve = screen.getByRole("button", { name: en.approvals.approve });
    expect(approve.parentElement).toHaveAttribute("title", en.approvals.needsRole);
    expect(approve).toHaveAccessibleDescription(en.approvals.needsRole);

    await userEvent.click(screen.getByRole("button", { name: en.approvals.approve }));
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("keeps an author from approving their own proposal (AG-11)", async () => {
    const fetchMock = renderDetail({
      change: proposal({ author: { name: "Jana Kováčová", email: APPROVER.email } }),
    });

    expect(await screen.findByRole("button", { name: en.approvals.approve })).toBeDisabled();
    expect(screen.getByText(en.approvals.ownProposal)).toBeInTheDocument();
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("lets an administrator of the kind approve their own proposal, and says so (PF-58)", async () => {
    const fetchMock = renderDetail({
      change: proposal({ author: { name: "Jana Kováčová", email: APPROVER.email } }),
      permissions: {
        project: "banskabystrica",
        bootstrap: false,
        grants: [
          {
            role: "org-admin",
            binding: "admins",
            rule: { kinds: ["Endpoint"], verbs: ["propose", "approve", "delete"] },
          },
        ],
      },
    });

    const approve = await screen.findByRole("button", { name: en.approvals.approve });
    await waitFor(() => expect(approve).toBeEnabled());
    expect(
      screen.getByText(i18n.t("approvals.ownAsAdministrator", { kind: "Endpoint" })),
    ).toBeInTheDocument();
    await userEvent.click(approve);
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(posts(fetchMock)[0].url).toContain("/changes/chg-1a2b3c4d/approve");
  });

  it("keeps an author who may approve but not delete the kind from approving their own proposal", async () => {
    const fetchMock = renderDetail({
      change: proposal({ author: { name: "Jana Kováčová", email: APPROVER.email } }),
      permissions: {
        project: "banskabystrica",
        bootstrap: false,
        grants: [
          {
            role: "steward",
            binding: "stewards",
            rule: { kinds: ["Endpoint"], verbs: ["propose", "approve"] },
          },
        ],
      },
    });

    expect(await screen.findByText(en.approvals.ownProposal)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.approvals.approve })).toBeDisabled();
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it("holds a red-lane approval until the resource name is typed back", async () => {
    const fetchMock = renderDetail({
      change: proposal({
        status: { lane: "red", phase: "PendingApproval", plan: { delete: 1 } },
      }),
    });

    expect(await screen.findByRole("button", { name: en.approvals.approve })).toBeDisabled();
    expect(screen.getByRole("button", { name: en.approvals.reject })).toBeEnabled();

    expect(screen.getByRole("button", { name: en.approvals.approve }).parentElement).toHaveAttribute(
      "title",
      en.approvals.confirmFirst.replace("{name}", "air-quality"),
    );
    const confirm = screen.getByLabelText(en.approvals.confirmLabel);
    await userEvent.type(confirm, "air-qualit");
    expect(screen.getByRole("button", { name: en.approvals.approve })).toBeDisabled();

    await userEvent.type(confirm, "y");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: en.approvals.approve })).toBeEnabled(),
    );

    await userEvent.click(screen.getByRole("button", { name: en.approvals.approve }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    await expect(posts(fetchMock)[0].clone().json()).resolves.toEqual({ confirm: "air-quality" });
  });
});

describe("the files of a bundle (T-0861)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-value";
    window.history.pushState({}, "", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists every file of the merge request, the strictest lane first", async () => {
    renderDetail({
      change: proposal({
        fileCount: 2,
        files: [
          {
            path: "projects/banskabystrica/pipelines/aq.yaml",
            kind: "Pipeline",
            operation: "Create",
            lane: "green",
          },
          {
            path: "users/assignments/mallory-admin.yaml",
            kind: "RoleBinding",
            operation: "Create",
            lane: "red",
          },
        ],
      }),
    });

    const rows = await screen.findAllByTestId("change-file");
    expect(rows).toHaveLength(2);
    // The one that decides the confirmation is read first.
    expect(rows[0]).toHaveTextContent("RoleBinding");
    expect(rows[0]).toHaveTextContent("users/assignments/mallory-admin.yaml");
    expect(rows[0]).toHaveTextContent(en.lane.red);
    expect(rows[1]).toHaveTextContent("Pipeline");
    expect(rows[1]).toHaveTextContent(en.approvals.diffAdded);
  });

  it("shows no file section for a change the API listed no files for", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: en.approvals.diffType });
    expect(screen.queryAllByTestId("change-file")).toHaveLength(0);
  });
});
