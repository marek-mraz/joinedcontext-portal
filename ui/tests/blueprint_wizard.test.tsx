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

const THRESHOLD_ALERT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Blueprint",
  metadata: {
    name: "threshold-alert",
    namespace: "org",
    title: { en: "Threshold Alert" },
    description: { en: "Calls a webhook when a property crosses a limit" },
  },
  spec: {
    version: "1.2.0",
    category: "alerting",
    riskClass: "green",
    allowedRoles: ["domain-editor"],
    parameterSchema: {
      type: "object",
      required: ["entityType", "thresholdValue", "webhookUrl"],
      properties: {
        entityType: {
          type: "string",
          title: "Target entity type",
          enum: ["AirQualityObserved", "NoiseLevelObserved"],
        },
        thresholdValue: { type: "number", title: "Alert threshold" },
        webhookUrl: { type: "string", title: "Notification target URL" },
      },
    },
  },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
};

function renderWizard(flowResponse: { body: unknown; status: number } = { body: CHANGE, status: 202 }) {
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
    if (path.endsWith("/flows")) {
      return json(flowResponse.body, flowResponse.status);
    }
    if (path.endsWith("/blueprints")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items: [THRESHOLD_ALERT],
      });
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

async function openWizard(user: ReturnType<typeof userEvent.setup>) {
  const card = (await screen.findByText("Threshold Alert")).closest("li") as HTMLElement;
  await user.click(within(card).getByRole("button", { name: en.flows.run }));
  await screen.findByRole("heading", { name: "Set up Threshold Alert" });
}

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request.method !== "GET");
}

describe("blueprint wizard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/flows");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("generates one field per parameter, with the widget the schema implies (CC-24, CC-31)", async () => {
    const user = userEvent.setup();
    renderWizard();
    await openWizard(user);

    // An enum is a select, a number is a number input; nothing here is hand-built per kind.
    const entityType = screen.getByLabelText(/Target entity type/);
    expect(entityType.tagName).toBe("SELECT");
    expect(within(entityType as HTMLSelectElement).getByRole("option", { name: "AirQualityObserved" }))
      .toBeInTheDocument();
    expect(screen.getByLabelText(/Alert threshold/)).toHaveAttribute("type", "number");
    expect(screen.getByLabelText(/Notification target URL/)).toBeInTheDocument();
  });

  it("submits the parameters, the blueprint and the version the form came from", async () => {
    const user = userEvent.setup();
    const fetchMock = renderWizard();
    await openWizard(user);

    await user.selectOptions(screen.getByLabelText(/Target entity type/), "AirQualityObserved");
    await user.type(screen.getByLabelText(/Alert threshold/), "50");
    await user.type(screen.getByLabelText(/Notification target URL/), "https://example.org/hook");
    await user.click(screen.getByRole("button", { name: en.flows.instantiate.submit }));

    await waitFor(() => {
      expect(writes(fetchMock)).toHaveLength(1);
    });
    const request = writes(fetchMock)[0];
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/flows");
    const body = JSON.parse(await request.text()) as Record<string, unknown>;
    expect(body.blueprint).toBe("threshold-alert");
    // CC-26: the server refuses a form filled against a version that has since moved on.
    expect(body.version).toBe("1.2.0");
    expect(body.parameters).toEqual({
      entityType: "AirQualityObserved",
      thresholdValue: 50,
      webhookUrl: "https://example.org/hook",
    });
  });

  it("does not submit while a required parameter is missing (CC-60)", async () => {
    const user = userEvent.setup();
    const fetchMock = renderWizard();
    await openWizard(user);

    await user.type(screen.getByLabelText(/Notification target URL/), "https://example.org/hook");
    await user.click(screen.getByRole("button", { name: en.flows.instantiate.submit }));

    await waitFor(() => {
      expect(screen.getAllByText(en.form.required).length).toBeGreaterThan(0);
    });
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("shows the change the submission produced rather than a saved record (CC-32)", async () => {
    const user = userEvent.setup();
    renderWizard();
    await openWizard(user);

    await user.selectOptions(screen.getByLabelText(/Target entity type/), "AirQualityObserved");
    await user.type(screen.getByLabelText(/Alert threshold/), "50");
    await user.type(screen.getByLabelText(/Notification target URL/), "https://example.org/hook");
    await user.click(screen.getByRole("button", { name: en.flows.instantiate.submit }));

    expect(await screen.findByText("chg-77aa11bb")).toBeInTheDocument();
    expect(screen.getByText(en.changes.accepted)).toBeInTheDocument();
  });

  it("shows the server's own reason when the flow is refused", async () => {
    const user = userEvent.setup();
    renderWizard({
      status: 503,
      body: {
        type: "https://joinedcontext.com/errors/service-unavailable",
        title: "Service Unavailable",
        status: 503,
        detail: "this build cannot expand blueprints",
      },
    });
    await openWizard(user);

    await user.selectOptions(screen.getByLabelText(/Target entity type/), "AirQualityObserved");
    await user.type(screen.getByLabelText(/Alert threshold/), "50");
    await user.type(screen.getByLabelText(/Notification target URL/), "https://example.org/hook");
    await user.click(screen.getByRole("button", { name: en.flows.instantiate.submit }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("this build cannot expand blueprints");
  });
});
