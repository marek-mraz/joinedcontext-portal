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
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["domain-editor"],
};

function app(overrides: Record<string, unknown> = {}, spec: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: {
      name: "mapa-ovzdusia",
      namespace: "banskabystrica",
      title: { en: "Air quality map" },
      description: { en: "Stations coloured by PM10" },
      ...overrides,
    },
    status: {
      phase: "Live",
      observedRevision: "9f1c2ab",
      sourceUrl: "https://git.example.sk/city/config/src/branch/app/mapa-ovzdusia",
    },
    spec: {
      kind: "static",
      visibility: "project",
      lifecycle: "preview",
      embeddable: true,
      dataNeeds: [
        {
          contextSpaceRef: { name: "ovzdusie" },
          types: ["AirQualityObserved"],
          operations: ["queryEntity"],
        },
      ],
      ...spec,
    },
  };
}

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000a1b2", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

function renderCatalog(
  apps: unknown[],
  writeResponse: { body: unknown; status: number } = { body: CHANGE, status: 202 },
) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
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
    if (path.endsWith("/apps") && request.method === "GET") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: apps });
    }
    if (request.method !== "GET") {
      return json(writeResponse.body, writeResponse.status);
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

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method !== "GET");
}

describe("apps catalog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/apps");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders one card per app with its lifecycle, visibility and data needs (AP-18)", async () => {
    renderCatalog([app(), app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" })]);

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    // The chip, not the button: the colour is never the only carrier, so it has the tooltip.
    expect(within(card).getByTitle(en.appLifecycle.previewHelp)).toHaveTextContent(
      en.appLifecycle.preview,
    );
    expect(within(card).getByText("Visible to project")).toBeInTheDocument();
    expect(within(card).getByText(/AirQualityObserved/)).toBeInTheDocument();
    expect(within(card).getByText(/ovzdusie/)).toBeInTheDocument();

    // AP-24: the prompt history and the branch live with the source, so the card links there.
    expect(within(card).getByRole("link", { name: en.apps.history })).toHaveAttribute(
      "href",
      "https://git.example.sk/city/config/src/branch/app/mapa-ovzdusia",
    );
  });

  it("offers preview and publish only while the app is in preview (AP-18, AP-19)", async () => {
    renderCatalog([app(), app({ name: "hluk", title: { en: "Noise" } }, { lifecycle: "published" })]);

    const preview = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    expect(within(preview).getByRole("button", { name: en.apps.previewAction })).toBeInTheDocument();
    expect(within(preview).getByRole("button", { name: en.apps.publishAction })).toBeInTheDocument();

    // A published app is reached by its own audience, and there is nothing left to publish.
    const published = screen.getByText("Noise").closest("li") as HTMLElement;
    expect(within(published).queryByRole("button", { name: en.apps.previewAction })).toBeNull();
    expect(within(published).queryByRole("button", { name: en.apps.publishAction })).toBeNull();
  });

  it("frames the preview in an opaque origin, never same-origin with the Portal (AP-19)", async () => {
    const user = userEvent.setup();
    renderCatalog([app()]);

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.apps.previewAction }));

    const frame = await screen.findByTitle("Preview of Air quality map");
    // An app is served from the Portal's own origin (AP-14), so `allow-same-origin` would let
    // the framed app act as the signed-in reviewer. `allow-scripts` alone is the whole point.
    const sandbox = frame.getAttribute("sandbox") ?? "";
    expect(sandbox.split(/\s+/)).toContain("allow-scripts");
    expect(sandbox).not.toContain("allow-same-origin");
    expect(sandbox).not.toContain("allow-top-navigation");
    // The commit the preview was built from, so a reviewer never reads a stale build.
    expect(frame).toHaveAttribute("src", "/apps/mapa-ovzdusia/?preview=9f1c2ab");
  });

  it("says why an app that refuses framing has no preview (AP-12)", async () => {
    const user = userEvent.setup();
    renderCatalog([app({}, { embeddable: false })]);

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.apps.previewAction }));

    expect(await screen.findByRole("alert")).toHaveTextContent(en.apps.preview.notEmbeddable);
    expect(screen.queryByTitle("Preview of Air quality map")).toBeNull();
  });

  it("publishing asks first, then proposes a change rather than flipping a switch (AP-20)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([app()]);

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.apps.publishAction }));

    // The confirmation says what publishing does, and nothing has been sent yet.
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("reachable by project");
    expect(writes(fetchMock)).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.confirm }));

    await waitFor(() => {
      expect(writes(fetchMock)).toHaveLength(1);
    });
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/apps/mapa-ovzdusia",
    );
    const body = JSON.parse(await request.text()) as { spec: Record<string, unknown> };
    expect(body.spec.lifecycle).toBe("published");
    // MF-04: the status is the API's own, and a write never sends it back.
    expect(body).not.toHaveProperty("status");

    // The answer is a merge request, not a saved record (CC-32).
    expect(await screen.findByText("chg-0000a1b2")).toBeInTheDocument();
    expect(screen.getByText(en.changes.accepted)).toBeInTheDocument();
  });

  it("cancelling the publication sends nothing", async () => {
    const user = userEvent.setup();
    const fetchMock = renderCatalog([app()]);

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.apps.publishAction }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.cancel }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("shows the server's own reason when the publication is refused", async () => {
    const user = userEvent.setup();
    renderCatalog([app()], {
      status: 403,
      body: {
        type: "https://joinedcontext.com/errors/forbidden",
        title: "Forbidden",
        status: 403,
        detail: "publishing a public app needs the org-admin role",
      },
    });

    const card = (await screen.findByText("Air quality map")).closest("li") as HTMLElement;
    await user.click(within(card).getByRole("button", { name: en.apps.publishAction }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: en.apps.publish.confirm }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "publishing a public app needs the org-admin role",
    );
  });

  it("an empty catalogue says so instead of showing an empty grid", async () => {
    renderCatalog([]);
    expect(await screen.findByText(en.apps.empty)).toBeInTheDocument();
  });
});
