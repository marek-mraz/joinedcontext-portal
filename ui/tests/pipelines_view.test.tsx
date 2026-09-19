import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { executionClass } from "../src/routes/PipelinesPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const PIPELINES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "aq-mqtt-ingest", namespace: "banskabystrica" },
      spec: {
        class: "resident",
        secretRefs: [
          // A manifest may never carry a value; if one is smuggled in, no view renders it.
          { name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD", value: "hunter2" },
        ],
      },
      status: {
        phase: "Live",
        sourceUrl:
          "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/pipelines/aq-mqtt-ingest/pipeline.yaml",
      },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "parking-daily", namespace: "banskabystrica" },
      spec: { class: "auto", period: "1h", enabled: false },
      status: { phase: "Live" },
    },
  ],
};

const METRICS = {
  pipeline: "aq-mqtt-ingest",
  scrapedAt: "2026-09-06T16:20:11Z",
  received: 128401,
  sent: 128390,
  errors: 3,
  latencyP99Ms: 42.5,
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

/** What the dry run before a write answers: green unless a test asks for a red check (PF-57). */
const GREEN = { valid: true, verdict: { ok: true, findings: [] } };

function renderPipelines(
  metrics: unknown = METRICS,
  metricsStatus = 200,
  pipelines: unknown = PIPELINES,
  check: unknown = GREEN,
) {
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
    if (request.method !== "GET") {
      // A write is checked on the same route and verb first (T-2264); only the real one answers
      // with the Change.
      return new URL(request.url).searchParams.get("dryRun") === "All"
        ? json(check)
        : json(CHANGE, 202);
    }
    if (path.endsWith("/metrics")) {
      return json(metrics, metricsStatus);
    }
    if (path.endsWith("/pipelines")) {
      return json(pipelines);
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

const rowOf = async (name: string) =>
  (await screen.findByText(name)).closest("tr") as HTMLElement;

describe("pipelines view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("classifies by the 30-second rule when the manifest says auto", () => {
    expect(executionClass({ class: "resident", period: "1h" })).toBe("resident");
    expect(executionClass({ class: "scheduled" })).toBe("scheduled");
    expect(executionClass({ class: "auto" })).toBe("resident");
    expect(executionClass({ class: "auto", period: "250ms" })).toBe("resident");
    expect(executionClass({ class: "auto", period: "29s" })).toBe("resident");
    expect(executionClass({ class: "auto", period: "30s" })).toBe("scheduled");
    expect(executionClass({ class: "auto", period: "5m" })).toBe("scheduled");
    expect(executionClass({ class: "auto", schedule: "10 0 * * *" })).toBe("scheduled");
  });

  /// T-1058: an empty list is where a person decides what to do next.
  it("says the two ways in when the project has no pipeline yet", async () => {
    renderPipelines(METRICS, 200, {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [],
    });

    expect(await screen.findByText(en.pipelines.empty)).toBeInTheDocument();
    expect(screen.getByText(en.pipelines.emptyHint)).toBeInTheDocument();
    // The way in is a button, in the empty state as well as in the header.
    expect(screen.getAllByRole("button", { name: en.pipelines.add }).length).toBeGreaterThan(1);
  });

  it("shows the execution class of every pipeline", async () => {
    renderPipelines();

    expect(within(await rowOf("aq-mqtt-ingest")).getByText(en.pipelines.class.resident))
      .toBeInTheDocument();
    expect(within(await rowOf("parking-daily")).getByText(en.pipelines.class.scheduled))
      .toBeInTheDocument();
  });

  it("shows the live counters of a resident stream", async () => {
    renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    expect(await within(row).findByText("128,401")).toBeInTheDocument();
    expect(within(row).getByText("3")).toBeInTheDocument();
    expect(within(row).getByText("42.5")).toBeInTheDocument();
  });

  it("names the secret reference but never a secret value (PL-17)", async () => {
    renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    expect(within(row).getByText(/mqtt-credentials\/password/)).toBeInTheDocument();
    expect(within(row).queryByText(/hunter2/)).not.toBeInTheDocument();
    expect(screen.queryByText(/hunter2/)).not.toBeInTheDocument();
  });

  it("keeps a runner that is down out of the pipeline's status", async () => {
    renderPipelines({ status: 503, title: "Service Unavailable" }, 503);

    const row = await rowOf("aq-mqtt-ingest");
    expect(await within(row).findByText(en.pipelines.metrics.unavailable)).toBeInTheDocument();
    // The manifest is still Live: a runner that is down is not a broken pipeline.
    expect(within(row).getByText(en.phase.live)).toBeInTheDocument();
  });

  it("does not poll a paused pipeline and offers to start it again", async () => {
    const fetchMock = renderPipelines();

    const row = await rowOf("parking-daily");
    expect(within(row).getByText(en.pipelines.metrics.paused)).toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some((call) =>
        new URL((call[0] as Request).url).pathname.includes("parking-daily/metrics"),
      ),
    ).toBe(false);

    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.resume }));

    await waitFor(() => {
      const write = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((request) => request.method === "PUT");
      expect(write?.url).toContain("/pipelines/parking-daily");
    });
  });

  it("proposes a change instead of stopping a running pipeline directly (CC-35)", async () => {
    document.cookie = "jc_csrf=csrf-token-value";
    const fetchMock = renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    const write = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((r) => r.method === "PUT");
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(write.headers.get("x-csrf-token")).toBe("csrf-token-value");
    const body = JSON.parse(await write.clone().text()) as {
      spec: { enabled: boolean };
      status?: unknown;
    };
    expect(body.spec.enabled).toBe(false);
    expect(body.status).toBeUndefined();
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
  });

  it("checks the manifest on the same route before it writes it (PF-57, T-2264)", async () => {
    const fetchMock = renderPipelines();

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    // Two requests, in this order: the dry run that records the verdict, then the write. Without
    // the check the verdict gate refuses the write with "The manifest has not been checked" and
    // the click does nothing at all.
    const writes = await waitFor(() => {
      const requests = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .filter((request) => request.method === "PUT");
      expect(requests).toHaveLength(2);
      return requests;
    });
    expect(new URL(writes[0].url).searchParams.get("dryRun")).toBe("All");
    expect(new URL(writes[1].url).searchParams.get("dryRun")).toBe(null);
    // The same manifest both times: a check of something else proves nothing about what lands.
    expect(await writes[0].clone().text()).toBe(await writes[1].clone().text());
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
  });

  it("writes nothing when the check is red, and says what it found", async () => {
    const fetchMock = renderPipelines(METRICS, 200, PIPELINES, {
      valid: false,
      verdict: { ok: false, findings: [{ message: "the runner has no room for another stream" }] },
    });

    const row = await rowOf("aq-mqtt-ingest");
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.pause }));

    expect(await screen.findByText(/the runner has no room for another stream/)).toBeInTheDocument();
    const writes = fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .filter((request) => request.method === "PUT");
    expect(writes).toHaveLength(1);
    expect(new URL(writes[0].url).searchParams.get("dryRun")).toBe("All");
  });
});
