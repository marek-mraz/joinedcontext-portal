import { render, screen, waitFor } from "@testing-library/react";
import axe from "axe-core";
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

function renderApp() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

async function expectNoViolations(container: HTMLElement) {
  const results = await axe.run(container);
  const summary = results.violations
    .map((v) => `${v.id}: ${v.description} (${v.nodes.map((n) => n.html).join("; ")})`)
    .join("\n");
  expect(results.violations, summary).toEqual([]);
}

describe("accessibility", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("sk");
    window.history.pushState({}, "", "/");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("the login route has no axe violations", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response(null, { status: 401 }))));

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /prihlásiť/i })).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });

  it("the authenticated shell has no axe violations", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url =
          typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
        const body = url.includes("/auth/me")
          ? IDENTITY
          : { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
        return Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }),
    );

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("banner")).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });
});
