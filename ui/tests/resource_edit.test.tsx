import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parse as parseYaml } from "yaml";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

// Monaco needs a canvas and a worker, which jsdom has neither of; a textarea keeps its contract.
function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />;
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

const { App } = await import("../src/App");

const PROJECT = "banskabystrica";
const NAME = "zvolen-ovzdusie";

const REGISTRATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSourceRegistration",
  metadata: { name: NAME, namespace: PROJECT, title: { en: "Zvolen air quality" } },
  spec: { contextSpaceRef: "hub", endpoint: "https://zvolen.example/ngsi-ld" },
  status: { phase: "Live" },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002b", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};

function renderList(options: { verbs: string[]; answer?: "change" | "invalid" }) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200, type = "application/json") =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": type } }));
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "jana.kovacova", roles: [] });
    }
    if (path.endsWith("/permissions/me")) {
      return json({ grants: [{ rule: { kinds: ["ContextSourceRegistration"], verbs: options.verbs } }] });
    }
    if (request.method === "PUT") {
      return options.answer === "invalid"
        ? json(
            {
              type: "https://joinedcontext.com/errors/bad-request",
              title: "Bad Request",
              status: 400,
              detail: "spec.endpoint must be an https URL",
            },
            400,
            "application/problem+json",
          )
        : json(CHANGE, 202);
    }
    if (path.endsWith(`/csrs/${NAME}`)) {
      return json(REGISTRATION);
    }
    if (path.endsWith("/csrs")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [REGISTRATION] });
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

const puts = (fetchMock: ReturnType<typeof vi.fn>) =>
  fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => request.method === "PUT");

async function openEditor() {
  await userEvent.click(await screen.findByRole("button", { name: "Edit Zvolen air quality" }));
  const dialog = await screen.findByRole("dialog");
  const yaml = await within(dialog).findByRole("textbox", { name: "YAML" });
  return { dialog, yaml: yaml as HTMLTextAreaElement };
}

describe("editing a resource from its list", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-value";
    window.history.pushState({}, "", `/projects/${PROJECT}/csrs`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows no Edit to a person whose role may not propose the kind", async () => {
    renderList({ verbs: ["delete"] });
    expect(await screen.findByText("Zvolen air quality")).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Edit Zvolen air quality" })).not.toBeInTheDocument();
    });
  });

  it("opens the stored manifest without its status and proposes the edit once", async () => {
    const fetchMock = renderList({ verbs: ["propose"] });
    const { dialog, yaml } = await openEditor();
    const shown = parseYaml(yaml.value) as Record<string, unknown>;
    expect(shown.status).toBeUndefined();
    expect(shown.spec).toEqual(REGISTRATION.spec);

    await userEvent.clear(yaml);
    await userEvent.type(yaml, `kind: ContextSourceRegistration\nmetadata:\n  name: ${NAME}\nspec:\n  contextSpaceRef: air\n`);
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));

    expect(await within(dialog).findByText(CHANGE.metadata.name)).toBeInTheDocument();
    const sent = puts(fetchMock);
    expect(sent).toHaveLength(1);
    expect(new URL(sent[0].url).pathname).toBe(`/api/v1/projects/${PROJECT}/csrs/${NAME}`);
    expect(sent[0].headers.get("x-csrf-token")).toBe("csrf-token-value");
    expect((await sent[0].json()).spec).toEqual({ contextSpaceRef: "air" });
  });

  it("refuses a renamed manifest before anything is sent", async () => {
    const fetchMock = renderList({ verbs: ["propose"] });
    const { dialog, yaml } = await openEditor();
    await userEvent.clear(yaml);
    await userEvent.type(yaml, "kind: ContextSourceRegistration\nmetadata:\n  name: banska-ovzdusie\n");
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(`Keep the name ${NAME}`);
    expect(puts(fetchMock)).toHaveLength(0);
  });

  it("shows why the platform refused the edit", async () => {
    renderList({ verbs: ["propose"], answer: "invalid" });
    const { dialog } = await openEditor();
    await userEvent.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("spec.endpoint must be an https URL");
  });

  it("opens the editor at once for a page opened to edit one resource", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/csrs?edit=${NAME}`);
    const fetchMock = renderList({ verbs: ["propose"] });
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Edit Zvolen air quality");
    expect(puts(fetchMock)).toHaveLength(0);
  });
});
