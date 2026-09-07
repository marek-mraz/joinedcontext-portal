/** T-0497: a Pipeline from a form or its YAML, proposed through the change flow (PL-04, PL-31, PL-33, PL-39, UI-01, AP-13). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parse as parseYaml } from "yaml";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom. The stand-in
// is a textarea with the same contract, so what the test exercises is the dialog's own work:
// the YAML it writes, the manifest it reads back and the validation between them.
function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return (
    <textarea
      aria-label="YAML"
      value={value}
      onChange={(event) => onChange?.(event.target.value)}
    />
  );
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

const { App } = await import("../src/App");
const { endpointUrn, fromManifest, toEnvelope, toForm } = await import(
  "../src/pages/pipelines/PipelineEditor"
);

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  // A plain editor, not an approver: the button is for everyone signed in.
  roles: ["portal-editor"],
};

const BRANDING = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Banská Bystrica",
  organisation: "Mesto Banská Bystrica",
  orgDomain: "banskabystrica.sk",
  domain: "portal.banskabystrica.sk",
  contactEmail: "data@banskabystrica.sk",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const EXISTING: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "aq-mqtt-ingest", namespace: "banskabystrica" },
  spec: {
    class: "resident",
    enabled: false,
    source: { dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } },
    compute: { kind: "bloblang" },
    targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }],
  },
  status: {
    phase: "Live",
    sourceUrl:
      "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/pipelines/aq-mqtt-ingest/pipeline.yaml",
  },
};

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const DATASOURCES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "mqtt-mesto", namespace: "banskabystrica" },
    spec: { type: "mqtt", mqtt: { urls: ["tls://mqtt.banskabystrica.sk:8883"], topics: ["aq/#"] } },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "banskabystrica" },
    spec: { contextSpaceRef: "ovzdusie", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

function renderPipelines() {
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
    if (path.endsWith("/branding")) {
      return json(BRANDING);
    }
    if (request.method !== "GET") {
      return json(CHANGE, 202);
    }
    if (path.endsWith("/pipelines")) {
      return json(list([EXISTING]));
    }
    if (path.endsWith("/datasources")) {
      return json(DATASOURCES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    return json(list([]));
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
    .filter((request) => request.method === "POST" || request.method === "PUT");
}

async function openNew() {
  await userEvent.click(await screen.findByRole("button", { name: en.pipelines.add }));
  const dialog = await screen.findByRole("dialog");
  // The selects are filled from the project's lists once they arrive.
  await within(dialog).findByRole("option", { name: "mqtt-mesto" });
  return dialog;
}

const yamlTab = (dialog: HTMLElement) => within(dialog).getByRole("tab", { name: en.form.view.yaml });
const formTab = (dialog: HTMLElement) => within(dialog).getByRole("tab", { name: en.form.view.form });
// Awaited: the editor is loaded lazily, so it is behind a Suspense boundary for a moment.
const editor = (dialog: HTMLElement) =>
  within(dialog).findByLabelText("YAML") as Promise<HTMLTextAreaElement>;

async function replaceYaml(dialog: HTMLElement, text: string) {
  const area = await editor(dialog);
  await userEvent.clear(area);
  // `paste`, not `type`: userEvent would take `{` and `[` in the text as key descriptors.
  await userEvent.click(area);
  await userEvent.paste(text);
}

describe("pipeline editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("maps a manifest to the form and back without losing a field", () => {
    const form = toForm(EXISTING);
    expect(form.source?.dataSourceRef).toBe("mqtt-mesto");
    expect(form).not.toHaveProperty("enabled");

    const envelope = toEnvelope("banskabystrica", form, EXISTING);
    expect(envelope.metadata).toEqual({ name: "aq-mqtt-ingest", namespace: "banskabystrica" });
    expect(envelope.spec).toEqual(EXISTING.spec);
    // What the YAML view shows is what the form reads back.
    expect(fromManifest(envelope)).toEqual(form);
  });

  it("builds the target URN from the branding's domain, the endpoint's space and its name", () => {
    const endpoint = ENDPOINTS.items[0] as Manifest;
    expect(endpointUrn("banskabystrica.sk", endpoint)).toBe(
      "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    );
    expect(endpointUrn("", endpoint)).toBeUndefined();
  });

  it("round-trips the form through the YAML view and keeps the manifest", async () => {
    renderPipelines();
    const dialog = await openNew();

    await userEvent.type(within(dialog).getByLabelText(/^Name/), "aq-derived");
    await userEvent.selectOptions(within(dialog).getByLabelText(/^Data source/), "mqtt-mesto");
    await userEvent.selectOptions(
      within(dialog).getByLabelText(/^Target endpoint/),
      "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    );

    await userEvent.click(yamlTab(dialog));
    const shown = parseYaml((await editor(dialog)).value) as ReturnType<typeof toEnvelope>;
    expect(shown.kind).toBe("Pipeline");
    expect(shown.metadata.name).toBe("aq-derived");
    expect(shown.spec).toEqual({
      class: "auto",
      source: { dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } },
      targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    });

    // Edited as text, read back into the form.
    await replaceYaml(
      dialog,
      (await editor(dialog)).value.replace("class: auto", "class: auto\n  period: 15s"),
    );
    await userEvent.click(formTab(dialog));
    expect(within(dialog).getByLabelText(/^Period/)).toHaveValue("15s");
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("aq-derived");

    await userEvent.click(yamlTab(dialog));
    expect(parseYaml((await editor(dialog)).value)).toEqual({
      ...shown,
      spec: { ...shown.spec, period: "15s" },
    });
  });

  it("keeps a YAML that does not parse in the editor and sends nothing", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();

    await userEvent.click(yamlTab(dialog));
    await replaceYaml(dialog, "metadata: [unclosed");
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(/does not parse/);
    expect(writes(fetchMock)).toHaveLength(0);

    // The form tab is refused too: the text stays where the problem is.
    await userEvent.click(formTab(dialog));
    expect(await editor(dialog)).toHaveValue("metadata: [unclosed");
  });

  it("refuses a manifest the schema refuses, from the YAML view as from the form", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();

    await userEvent.click(yamlTab(dialog));
    // A scheduled pipeline without a schedule, and a source naming both inputs (PL-39).
    await replaceYaml(
      dialog,
      [
        "apiVersion: joinedcontext.com/v1alpha1",
        "kind: Pipeline",
        "metadata:",
        "  name: nightly",
        "spec:",
        "  class: scheduled",
        "  source:",
        "    dataSourceRef: mqtt-mesto",
        "    endpointRef: public-air",
        "  targetEndpoint: urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
      ].join("\n"),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(en.form.schemaErrors);
    expect(alert).toHaveTextContent(en.form.required);
    expect(alert).toHaveTextContent(en.form.exclusive);
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("creates a pipeline with a POST carrying the manifest, and shows the change", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();

    await userEvent.type(within(dialog).getByLabelText(/^Name/), "aq-derived");
    await userEvent.selectOptions(within(dialog).getByLabelText(/^Execution/), "resident");
    await userEvent.selectOptions(within(dialog).getByLabelText(/^Data source/), "mqtt-mesto");
    await userEvent.selectOptions(
      within(dialog).getByLabelText(/^Target endpoint/),
      "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/pipelines");
    await expect(request.clone().json()).resolves.toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "aq-derived", namespace: "banskabystrica" },
      spec: {
        class: "resident",
        source: { dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } },
        targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
      },
    });
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("says where the Bloblang of a bloblang step lives, and asks wasm for its module", async () => {
    renderPipelines();
    const dialog = await openNew();

    expect(within(dialog).queryByText(en.pipelines.bloblangHint)).not.toBeInTheDocument();
    await userEvent.selectOptions(within(dialog).getByLabelText(/^Kind/), "bloblang");
    expect(within(dialog).getByText(en.pipelines.bloblangHint)).toBeInTheDocument();

    await userEvent.selectOptions(within(dialog).getByLabelText(/^Kind/), "wasm");
    expect(within(dialog).queryByText(en.pipelines.bloblangHint)).not.toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText(/^Name/), "aq-index");
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.propose }));
    // wasm needs module and function (PL-33): two required errors on the compute group.
    const alerts = await within(dialog).findAllByRole("alert");
    expect(alerts.filter((alert) => alert.textContent?.includes(en.form.required)).length)
      .toBeGreaterThanOrEqual(2);
  });

  it("edits an existing pipeline at its own path, keeping what the form does not show", async () => {
    const fetchMock = renderPipelines();

    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.pipelines.edit }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("aq-mqtt-ingest");
    expect(within(dialog).getByLabelText(/^Name/)).toHaveAttribute("readonly");
    expect(within(dialog).getByText(en.pipelines.bloblangHint)).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: /bento\.yaml/ })).toHaveAttribute(
      "href",
      expect.stringMatching(/pipelines\/aq-mqtt-ingest\/bento\.yaml$/),
    );

    await userEvent.type(within(dialog).getByLabelText(/^Period/), "10s");
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/pipelines/aq-mqtt-ingest",
    );
    const body = (await request.clone().json()) as { spec: Record<string, unknown>; status?: unknown };
    expect(body.spec).toEqual({ ...EXISTING.spec, period: "10s" });
    expect(body.status).toBeUndefined();
  });
});
