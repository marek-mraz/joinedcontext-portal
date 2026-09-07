import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const EMPTY_LIST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [],
};

/** What the dev repository holds: two `projects/<slug>/` directories. */
const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [{ name: "helsinki" }, { name: "banskabystrica" }],
};

describe("portal shell", () => {
  let client: QueryClient;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/");
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? PROJECTS
          : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    // The index route shows a bare page until the project list is in; the shell is up once
    // its sidebar is.
    await waitFor(() => {
      expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("exposes the ARIA landmarks a keyboard user navigates by", () => {
    expect(screen.getByRole("banner")).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeInTheDocument();
    expect(screen.getByRole("main")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#main");
  });

  it("lists every section of the resource API in the sidebar", () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    for (const label of [
      "Flows",
      "Context Spaces",
      "Endpoints",
      "Pipelines",
      "Dashboards",
      "Applications",
      "Approvals",
      "Access",
    ]) {
      expect(within(nav).getByRole("link", { name: label })).toBeInTheDocument();
    }
  });

  it("marks the section the router is on with aria-current", async () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    expect(within(nav).getByRole("link", { name: "Context Spaces" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await userEvent.click(within(nav).getByRole("link", { name: "Endpoints" }));

    await waitFor(() => {
      expect(within(nav).getByRole("link", { name: "Endpoints" })).toHaveAttribute(
        "aria-current",
        "page",
      );
    });
    expect(within(nav).getByRole("link", { name: "Context Spaces" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("navigating a section asks the API for that plural", async () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    await userEvent.click(within(nav).getByRole("link", { name: "Dashboards" }));

    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as RequestInfo | URL;
        return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      });
      expect(urls.some((url) => url.includes("/projects/helsinki/dashboards"))).toBe(true);
    });
  });

  it("shows the project and the active section as a breadcrumb trail", async () => {
    // A route change remounts the shell, so the breadcrumb node has to be looked up again.
    const crumbs = () => screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs()).getByRole("link", { name: "helsinki" })).toBeInTheDocument();
    expect(within(crumbs()).getByText("Context Spaces")).toHaveAttribute("aria-current", "page");

    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    await userEvent.click(within(nav).getByRole("link", { name: "Approvals" }));

    await waitFor(() => {
      expect(within(crumbs()).getByText("Approvals")).toHaveAttribute("aria-current", "page");
    });
    expect(within(crumbs()).queryByText("Context Spaces")).not.toBeInTheDocument();
  });

  it("offers the active project in a switcher and the identity in a user menu", () => {
    expect(screen.getByRole("button", { name: "Projects" })).toHaveTextContent("helsinki");
    expect(
      screen.getByRole("button", { name: "Signed in as Jana Kováčová" }),
    ).toBeInTheDocument();
  });

  it("lists every project the API holds and switches to the one picked", async () => {
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Projects" }));

    const items = await screen.findAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual(["helsinki", "banskabystrica"]);

    await user.click(screen.getByRole("menuitem", { name: "banskabystrica" }));

    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/banskabystrica/spaces");
    });
    expect(screen.getByRole("button", { name: "Projects" })).toHaveTextContent("banskabystrica");
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getByRole("link", { name: "banskabystrica" })).toBeInTheDocument();
    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((call) => {
        const input = call[0] as RequestInfo | URL;
        return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      });
      expect(urls.some((url) => url.includes("/projects/banskabystrica/spaces"))).toBe(true);
    });
  });

  it("offers every endpoint of every project under one sidebar entry", async () => {
    const nav = screen.getByRole("navigation", { name: "Main navigation" });
    await userEvent.click(within(nav).getByRole("link", { name: "All endpoints" }));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/endpoints");
    });
    expect(within(nav).getByRole("link", { name: "All endpoints" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });
});
