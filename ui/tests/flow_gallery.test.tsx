import { render, screen, within } from "@testing-library/react";
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
  roles: ["domain-editor"],
};

function blueprint(
  name: string,
  title: string,
  category: string | null,
  riskClass: string,
): Record<string, unknown> {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Blueprint",
    metadata: {
      name,
      namespace: "org",
      title: { en: title, sk: `${title} (sk)` },
      description: { en: `What ${name} does`, sk: "Popis" },
    },
    spec: {
      version: "1.2.0",
      category,
      riskClass,
      allowedRoles: ["domain-editor"],
      parameterSchema: {
        type: "object",
        required: ["webhookUrl"],
        properties: { webhookUrl: { type: "string", title: "Notification target URL" } },
      },
    },
  };
}

/** What the server returns: it has already dropped everything this caller may not run. */
const GALLERY = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    blueprint("threshold-alert", "Threshold Alert", "alerting", "green"),
    blueprint("onboard-source", "Onboard a data source", "onboarding", "yellow"),
    blueprint("share-with-city", "Share with another city", null, "red"),
  ],
};

function renderGallery(gallery: unknown = GALLERY) {
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
    if (path.endsWith("/blueprints")) {
      return json(gallery);
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

describe("flow gallery", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/flows");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows one card per blueprint with its review lane (CC-30, CC-59)", async () => {
    renderGallery();

    const card = (await screen.findByText("Threshold Alert")).closest("li") as HTMLElement;
    expect(within(card).getByText(en.flows.risk.green)).toBeInTheDocument();
    expect(within(card).getByText("What threshold-alert does")).toBeInTheDocument();
    expect(within(card).getByText("Version 1.2.0")).toBeInTheDocument();

    const risky = (await screen.findByText("Share with another city")).closest("li") as HTMLElement;
    expect(within(risky).getByText(en.flows.risk.red)).toBeInTheDocument();
  });

  it("filters the gallery by category and back again (CC-30)", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findByText("Threshold Alert");

    await user.click(screen.getByRole("button", { name: "onboarding" }));
    expect(screen.getByText("Onboard a data source")).toBeInTheDocument();
    expect(screen.queryByText("Threshold Alert")).not.toBeInTheDocument();

    // A blueprint with no category is still reachable, under its own chip.
    await user.click(screen.getByRole("button", { name: en.flows.category.uncategorised }));
    expect(screen.getByText("Share with another city")).toBeInTheDocument();
    expect(screen.queryByText("Onboard a data source")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: en.flows.category.all }));
    expect(screen.getByText("Threshold Alert")).toBeInTheDocument();
    expect(screen.getByText("Onboard a data source")).toBeInTheDocument();
  });

  it("asks the organization-level route, not a project collection", async () => {
    const fetchMock = renderGallery();
    await screen.findByText("Threshold Alert");

    const urls = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(urls).toContain("/api/v1/blueprints");
    expect(urls.some((url) => url.includes("/blueprints") && url.includes("/projects/"))).toBe(
      false,
    );
  });

  it("opens the wizard for the blueprint whose card was clicked (CC-30)", async () => {
    const user = userEvent.setup();
    renderGallery();

    const card = (await screen.findByText("Threshold Alert")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.flows.run }));

    expect(
      screen.getByRole("heading", { name: "Set up Threshold Alert" }),
    ).toBeInTheDocument();
    // The form is generated from the blueprint's own parameter schema (CC-24, CC-31).
    expect(screen.getByLabelText(/Notification target URL/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: en.flows.back }));
    expect(screen.getByText("Onboard a data source")).toBeInTheDocument();
  });

  it("says the gallery is empty rather than showing an empty grid", async () => {
    renderGallery({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    expect(await screen.findByText(en.flows.empty)).toBeInTheDocument();
  });

  it("shows the blueprint titles in the active locale", async () => {
    await i18n.changeLanguage("sk");
    renderGallery();
    expect(await screen.findByText("Threshold Alert (sk)")).toBeInTheDocument();
  });
});
