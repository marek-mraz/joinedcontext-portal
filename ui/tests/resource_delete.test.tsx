import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const PROJECT = "banskabystrica";
const NAME = "zvolen-ovzdusie";

const REGISTRATIONS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSourceRegistration",
      metadata: { name: NAME, namespace: PROJECT, title: { en: "Zvolen air quality" } },
      spec: { contextSpaceRef: "hub" },
      status: { phase: "Live" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002a", namespace: PROJECT },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 0, delete: 1 } },
};

const may = (verbs: string[]) => ({
  grants: [{ rule: { kinds: ["ContextSourceRegistration"], verbs } }],
});

function renderList(options: { verbs: string[]; answer?: "change" | "referenced" }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } }));
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "jana.kovacova", roles: [] });
    }
    if (path.endsWith("/permissions/me")) {
      return json(may(options.verbs));
    }
    if (request.method === "DELETE") {
      return options.answer === "referenced"
        ? json(
            {
              type: "https://joinedcontext.com/errors/conflict",
              title: "Conflict",
              status: 409,
              detail: "1 dependent resource blocks deletion: Endpoint live-traffic",
            },
            409,
            "application/problem+json",
          )
        : json(CHANGE, 202);
    }
    if (path.endsWith("/csrs")) {
      return json(REGISTRATIONS);
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

const deletes = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method === "DELETE");

describe("removing a resource from its list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-value";
    window.history.pushState({}, "", `/projects/${PROJECT}/csrs`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("keeps Delete disabled with the reason for a person whose role may not delete the kind (UI-44)", async () => {
    renderList({ verbs: ["propose"] });
    expect(await screen.findByText("Zvolen air quality")).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Delete Zvolen air quality" })).toBeDisabled();
    });
    expect(screen.getByRole("button", { name: "Delete Zvolen air quality" }).closest("[title]")?.getAttribute("title")).toMatch(
      /delete/,
    );
  });

  it("proposes the removal only once the name is typed back, and shows the change", async () => {
    const fetchMock = renderList({ verbs: ["propose", "delete"] });
    await userEvent.click(await screen.findByRole("button", { name: "Delete Zvolen air quality" }));

    const dialog = await screen.findByRole("dialog");
    const propose = within(dialog).getByRole("button", { name: en.resourceDelete.propose });
    expect(propose).toBeDisabled();
    const field = within(dialog).getByLabelText(`Type ${NAME} to confirm`);
    await userEvent.type(field, "zvolen-ovzdusi");
    expect(propose).toBeDisabled();
    await userEvent.type(field, "e");
    expect(propose).toBeEnabled();

    await userEvent.click(propose);
    expect(await within(dialog).findByText(CHANGE.metadata.name)).toBeInTheDocument();
    expect(within(dialog).getByText(en.changes.accepted)).toBeInTheDocument();
    const sent = deletes(fetchMock);
    expect(sent).toHaveLength(1);
    expect(new URL(sent[0].url).pathname).toBe(`/api/v1/projects/${PROJECT}/csrs/${NAME}`);
    expect(sent[0].headers.get("x-csrf-token")).toBe("csrf-token-value");
  });

  /// T-1054: the dialog exists to have a name typed into it, so that is where the caret goes.
  it("opens with the caret in the field the dialog is for", async () => {
    renderList({ verbs: ["propose", "delete"] });
    await userEvent.click(await screen.findByRole("button", { name: "Delete Zvolen air quality" }));

    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByLabelText(`Type ${NAME} to confirm`);
    await waitFor(() => {
      expect(document.activeElement).toBe(field);
    });
  });

  it("names what still references the resource when the removal is refused", async () => {
    renderList({ verbs: ["delete"], answer: "referenced" });
    await userEvent.click(await screen.findByRole("button", { name: "Delete Zvolen air quality" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(`Type ${NAME} to confirm`), NAME);
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceDelete.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(en.resourceDelete.referenced);
    expect(alert).toHaveTextContent("Endpoint live-traffic");
  });

  it("opens the confirmation at once for a page opened to remove one resource", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/csrs?delete=${NAME}`);
    const fetchMock = renderList({ verbs: ["delete"] });
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Remove Zvolen air quality");
    expect(deletes(fetchMock)).toHaveLength(0);
  });
});
