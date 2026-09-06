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

const CHANGES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeList",
  items: [
    {
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
      planFields: [
        { path: "spec.audience", from: "public", to: "internal" },
        { path: "spec.credentials.token", from: "[REDACTED]", to: "[REDACTED]" },
      ],
    },
  ],
};

function stubChangesApi() {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/changes")
          ? CHANGES
          : CHANGES.items[0];
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }),
  );
}

function stubManagerApi() {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const path = new URL(url, window.location.origin).pathname;
      let body: unknown = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
      if (path.endsWith("/auth/me")) {
        body = IDENTITY;
      } else if (path.endsWith("/spaces")) {
        body = {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "ContextSpace",
              metadata: { name: "ovzdusie", namespace: "banskabystrica" },
              spec: { dataModelRef: "AirQualityObserved" },
              status: { phase: "Live", sourceUrl: "https://git.example.sk/bb/org/src/branch/main/x.yaml" },
            },
          ],
        };
      } else if (path.endsWith("/dashboards")) {
        body = {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Dashboard",
              metadata: { name: "ovzdusie-prehlad", namespace: "banskabystrica" },
              spec: {
                title: { sk: "Kvalita ovzdušia", en: "Air quality" },
                visibility: "project",
                pages: [{ title: "Mapa", layout: "full-map", layers: [] }],
              },
              status: { phase: "Live" },
            },
          ],
        };
      } else if (path.endsWith("/pipelines")) {
        body = {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Pipeline",
              metadata: { name: "aq-mqtt-ingest", namespace: "banskabystrica" },
              spec: {
                class: "resident",
                secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }],
              },
              status: { phase: "Live" },
            },
          ],
        };
      } else if (path.endsWith("/blueprints")) {
        body = {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Blueprint",
              metadata: {
                name: "threshold-alert",
                namespace: "org",
                title: { sk: "Notifikácia prekročenia limitu", en: "Threshold Alert" },
                description: { sk: "Sleduje vlastnosť a zavolá webhook", en: "Watches a property" },
              },
              spec: {
                version: "1.2.0",
                category: "alerting",
                riskClass: "green",
                allowedRoles: ["domain-editor"],
                parameterSchema: {
                  type: "object",
                  required: ["webhookUrl"],
                  properties: { webhookUrl: { type: "string", title: "Webhook" } },
                },
              },
            },
          ],
        };
      } else if (path.endsWith("/endpoints")) {
        body = {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Endpoint",
              metadata: { name: "public-air", namespace: "banskabystrica" },
              spec: {
                contextSpaceRef: "ovzdusie",
                slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
                audience: "public",
                enabledRepresentations: ["ngsi-ld"],
              },
              status: { phase: "Live" },
            },
          ],
        };
      }
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }),
  );
}

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
  it("the approvals queue has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/approvals");
    stubChangesApi();

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("table")).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });

  it("the change under review has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/approvals/chg-1a2b3c4d");
    stubChangesApi();

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Schváliť" })).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });
  it("the Flow gallery has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/flows");
    stubManagerApi();

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 2 })).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });

  it("the Context Spaces manager has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces");
    stubManagerApi();

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("table")).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });

  it("the Pipelines manager has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
    stubManagerApi();

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("table")).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });

  it("the map dashboard has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/dashboards");
    stubManagerApi();

    const { container } = renderApp();
    // jsdom has no WebGL, so the map falls back to its message; the page around it is
    // what axe has to be clean on either way.
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });

  it("the Endpoints manager has no axe violations", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
    stubManagerApi();

    const { container } = renderApp();
    await waitFor(() => {
      expect(screen.getByRole("table")).toBeInTheDocument();
    });

    await expectNoViolations(container);
  });
});
