/**
 * Project → Access → People and roles (T-0740, AG-77, PF-52): who holds which role here, a grant
 * proposed from a form the assistant may have filled, a refusal shown in the API's words, and the
 * removal of a binding with its name typed back, all against the organization's routes.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { rememberPrefill } from "../src/assistant/state";
import { answeringChecks, checksSoFar } from "./checks";

const PROJECT = "helsinki";

const binding = (name: string, subject: Record<string, string>, role: string, scope: Record<string, string>) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "RoleBinding",
  metadata: { name, namespace: "org" },
  spec: { subjects: [subject], role, scope },
});

const BINDINGS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    binding("admins", { user: "demo.steward@hel.fi" }, "org-admin", { organization: "hel" }),
    binding("bikes-team", { group: "bikes" }, "pipeline-editor", { contextSpace: "citybikes" }),
    binding("espoo-stewards", { user: "someone@espoo.fi" }, "steward", { project: "espoo" }),
  ],
};

const ROLES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: ["org-admin", "pipeline-editor", "steward"].map((name) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Role",
    metadata: { name, namespace: "org" },
    spec: { rules: [] },
  })),
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "citybikes", namespace: PROJECT }, spec: {} },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-00000301", namespace: "org" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

const ADMIN = [{ rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }];

function renderAccess(options: { grants: unknown[]; refusal?: string }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } }));
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "demo.steward", email: "demo.steward@hel.fi", roles: [] });
    }
    if (path.endsWith("/permissions/me")) {
      return json({ grants: options.grants });
    }
    if (request.method === "POST" || request.method === "DELETE") {
      return options.refusal
        ? json(
            { type: "https://joinedcontext.com/errors/forbidden", title: "Forbidden", status: 403, detail: options.refusal },
            403,
            "application/problem+json",
          )
        : json(CHANGE, 202);
    }
    if (path === "/api/v1/projects/org/rolebindings") {
      return json(BINDINGS);
    }
    if (path === "/api/v1/projects/org/roles") {
      return json(ROLES);
    }
    if (path === `/api/v1/projects/${PROJECT}/spaces`) {
      return json(SPACES);
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

const writes = (fetchMock: ReturnType<typeof renderAccess>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method !== "GET");

const roles = () => screen.getByRole("table", { name: en.access.roles.caption.replace("{project}", PROJECT) });

describe("people and roles", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-value";
    window.history.pushState({}, "", `/projects/${PROJECT}/access`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    window.history.pushState({}, "", "/");
  });

  it("lists the roles held over the organization, this project and its spaces, not another project's", async () => {
    renderAccess({ grants: ADMIN });

    expect(await screen.findByText("demo.steward@hel.fi")).toBeInTheDocument();
    const table = roles();
    expect(within(table).getByText(en.access.roles.organization)).toBeInTheDocument();
    expect(within(table).getByText("Group bikes")).toBeInTheDocument();
    expect(within(table).getByText("Context space citybikes")).toBeInTheDocument();
    expect(within(table).queryByText("someone@espoo.fi")).not.toBeInTheDocument();
    expect(within(table).getAllByRole("button", { name: /^Edit\b/ })).toHaveLength(2);
    expect(within(table).getAllByRole("button", { name: /^Delete\b/ })).toHaveLength(2);
  });

  it("offers a viewer no change and keeps Grant a role where it is, disabled with the reason", async () => {
    renderAccess({ grants: [] });

    expect(await screen.findByText("demo.steward@hel.fi")).toBeInTheDocument();
    const grant = screen.getByRole("button", { name: en.access.roles.grant });
    expect(grant).toBeDisabled();
    expect(grant.closest("[title]")?.getAttribute("title")).toMatch(/RoleBinding/);
    expect(within(roles()).queryByRole("button", { name: /^(Edit|Delete)\b/ })).not.toBeInTheDocument();
  });

  it("proposes the binding the form shows to the organization's route and shows the red change", async () => {
    const fetchMock = renderAccess({ grants: ADMIN });

    await userEvent.click(await screen.findByRole("button", { name: en.access.roles.grant }));
    const dialog = await screen.findByRole("dialog");
    const propose = within(dialog).getByRole("button", { name: en.access.roles.propose });
    expect(propose).toBeDisabled();
    await userEvent.type(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel)), "jana.kovacova@hel.fi");
    await waitFor(() => expect(within(dialog).getByRole("option", { name: "steward" })).toBeInTheDocument());
    await userEvent.selectOptions(within(dialog).getByLabelText(new RegExp(en.access.roles.roleLabel)), "steward");
    await userEvent.selectOptions(within(dialog).getByLabelText(en.access.roles.whereLabel), "space:citybikes");
    await userEvent.click(propose);

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [post] = writes(fetchMock);
    expect(new URL(post.url).pathname).toBe("/api/v1/projects/org/rolebindings");
    await expect(post.clone().json()).resolves.toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "RoleBinding",
      metadata: { name: "jana-kovacova-steward-citybikes", namespace: "org" },
      spec: {
        subjects: [{ user: "jana.kovacova@hel.fi" }],
        role: "steward",
        scope: { contextSpace: "citybikes" },
      },
    });
    expect(await within(dialog).findByText("chg-00000301")).toBeInTheDocument();
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("POST /api/v1/projects/org/rolebindings"))).toBe(true);
  });

  it("opens the grant the assistant drafted, filled in, and shows a refusal in the API's words", async () => {
    const refusal = "a binding may not grant more than its proposer holds: missing delete on Endpoint (PF-52)";
    window.history.pushState({}, "", `/projects/${PROJECT}/access?grant=jana-kovacova-org-admin-helsinki`);
    rememberPrefill(`/projects/${PROJECT}/access?grant=jana-kovacova-org-admin-helsinki`, {
      ...binding("jana-kovacova-org-admin-helsinki", { user: "jana.kovacova" }, "org-admin", { project: PROJECT }),
    });
    renderAccess({ grants: ADMIN, refusal });

    const dialog = await screen.findByRole("dialog", { name: en.access.roles.grantTitle });
    expect(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel))).toHaveValue("jana.kovacova");
    await waitFor(() => expect(within(dialog).getByLabelText(new RegExp(en.access.roles.roleLabel))).toHaveValue("org-admin"));
    expect(within(dialog).getByLabelText(en.access.roles.whereLabel)).toHaveValue("project");

    await userEvent.click(within(dialog).getByRole("button", { name: en.access.roles.propose }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(refusal);
  });

  it("opens the grant the assistant drafts while the person is already on the access page", async () => {
    renderAccess({ grants: ADMIN });
    await screen.findByText("demo.steward@hel.fi");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // What the dock does with a navigate event: the form in hand, then the route with the draft.
    const route = `/projects/${PROJECT}/access?grant=jana-kovacova-steward-helsinki`;
    rememberPrefill(route, binding("jana-kovacova-steward-helsinki", { user: "jana.kovacova" }, "steward", { project: PROJECT }));
    act(() => {
      window.history.pushState({}, "", `${route}&draft=jana-kovacova-steward-helsinki`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    });

    const dialog = await screen.findByRole("dialog", { name: en.access.roles.grantTitle });
    expect(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel))).toHaveValue("jana.kovacova");
    await waitFor(() => expect(within(dialog).getByLabelText(new RegExp(en.access.roles.roleLabel))).toHaveValue("steward"));
  });

  it("removes a binding only after its name is typed back, through the organization's route", async () => {
    const fetchMock = renderAccess({ grants: ADMIN });

    await screen.findByText("demo.steward@hel.fi");
    const [remove] = within(roles()).getAllByRole("button", { name: /^Delete\b/ });
    await userEvent.click(remove);
    const dialog = await screen.findByRole("dialog");
    const confirm = within(dialog).getByRole("button", { name: en.resourceDelete.propose });
    await userEvent.type(within(dialog).getByRole("textbox"), "admin");
    expect(confirm).toBeDisabled();
    await userEvent.type(within(dialog).getByRole("textbox"), "s");
    await userEvent.click(confirm);

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    expect(new URL(writes(fetchMock)[0].url).pathname).toBe("/api/v1/projects/org/rolebindings/admins");
  });
});
