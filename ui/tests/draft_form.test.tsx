import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { digestOf } from "../src/api/digest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { useState } from "react";
import { BrandingProvider } from "../src/branding";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { ResourceFormDialogProps } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";

/** The dialog the way a page mounts it: the page owns the form data (UI-47). */
function Harness(
  props: Omit<ResourceFormDialogProps<TestFormData>, "formData" | "onChange">,
) {
  const [formData, setFormData] = useState<TestFormData | undefined>(undefined);
  return (
    <ResourceFormDialog<TestFormData>
      {...props}
      formData={formData}
      onChange={setFormData}
    />
  );
}

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, ((event: MessageEvent) => void)[]> = {};
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(event: string, handler: (event: MessageEvent) => void) {
    this.listeners[event] = this.listeners[event] || [];
    this.listeners[event].push(handler);
  }

  removeEventListener(event: string, handler: (event: MessageEvent) => void) {
    this.listeners[event] = (this.listeners[event] || []).filter(
      (h) => h !== handler,
    );
  }

  close() {
    MockEventSource.instances = MockEventSource.instances.filter(
      (i) => i !== this,
    );
  }

  emit(event: string, data: unknown) {
    const msg = { data: JSON.stringify(data) } as MessageEvent;
    if (this.listeners[event]) {
      for (const h of this.listeners[event]) {
        h(msg);
      }
    }
    if (this.listeners["message"]) {
      for (const h of this.listeners["message"]) {
        h(msg);
      }
    }
    if (this.onmessage) {
      this.onmessage(msg);
    }
  }
}

interface TestFormData {
  name?: string;
  url?: string;
}

const TEST_SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name" },
    url: { type: "string", title: "URL" },
  },
};

const TEST_SOURCE = {
  toManifest: (form: TestFormData) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: {
      name: form.name ?? "",
      namespace: "banskabystrica",
    },
    spec: {
      type: "http",
      http: { url: form.url ?? "" },
    },
  }),
  fromManifest: (manifest: unknown) => {
    const m = manifest as {
      metadata?: { name?: string };
      spec?: { http?: { url?: string } };
    };
    return {
      name: m.metadata?.name,
      url: m.spec?.http?.url,
    };
  },
};

describe("ResourceFormDialog shared drafts and verdict gates (AG-61, AG-62, UI-47, UI-48)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    MockEventSource.instances = [];
    vi.stubGlobal("EventSource", MockEventSource);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("writes a draft after typing a name with 600ms debounce", async () => {
    vi.useFakeTimers();
    const putCalls: { url: string; body: unknown }[] = [];

    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : input.toString();
        const method =
          input instanceof Request ? input.method : (init?.method ?? "GET");

        if (url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), {
            status: 200,
          });
        }
        if (
          method === "PUT" &&
          url.includes(
            "/api/v1/projects/banskabystrica/drafts/DataSource/new-source",
          )
        ) {
          const body = JSON.parse(await (input as Request).clone().text());
          putCalls.push({ url, body });
          return new Response(
            JSON.stringify({
              project: "banskabystrica",
              kind: "DataSource",
              name: "new-source",
              manifest: body.manifest,
              verdict: null,
              touchedBy: "demo.steward",
              touchedKind: "person",
              version: 1,
              updatedAt: "2026-09-13T12:00:00Z",
            }),
            { status: 200 },
          );
        }
        return new Response(JSON.stringify({}), { status: 200 });
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Create Data Source"
            description="Create draft"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const nameInput = screen.getByLabelText(/Name/i);
    fireEvent.change(nameInput, { target: { value: "new-source" } });

    // Immediately after typing, debounced call should not have triggered yet
    expect(putCalls).toHaveLength(0);

    // Fast-forward beyond the 600ms debounce
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700);
    });

    expect(putCalls).toHaveLength(1);
    expect(putCalls[0].url).toContain(
      "/api/v1/projects/banskabystrica/drafts/DataSource/new-source",
    );
    expect(putCalls[0].body).toMatchObject({
      manifest: {
        kind: "DataSource",
        metadata: { name: "new-source" },
      },
    });
  });

  it("writes a draft for a named resource that has no draft yet (the load answered 404)", async () => {
    vi.useFakeTimers();
    const putCalls: string[] = [];
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : input.toString();
        const method =
          input instanceof Request ? input.method : (init?.method ?? "GET");
        if (url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), {
            status: 200,
          });
        }
        if (url.includes("/drafts/Endpoint/bikes-regional")) {
          if (method === "GET") {
            return new Response(JSON.stringify({ detail: "no draft" }), {
              status: 404,
            });
          }
          putCalls.push(url);
          return new Response(
            JSON.stringify({
              project: "banskabystrica",
              kind: "Endpoint",
              name: "bikes-regional",
              manifest: {},
              verdict: null,
              touchedBy: "demo.steward",
              touchedKind: "person",
              version: 1,
              updatedAt: "2026-09-13T12:00:00Z",
            }),
            { status: 200 },
          );
        }
        return new Response(JSON.stringify({}), { status: 200 });
      },
    );
    vi.stubGlobal("fetch", fetchMock);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Create endpoint"
            description="Create draft"
            project="banskabystrica"
            draftKind="Endpoint"
            draftName="bikes-regional"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    fireEvent.change(screen.getByLabelText(/URL/i), {
      target: { value: "https://example.org/bikes" },
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700);
    });
    expect(putCalls).toHaveLength(1);
  });

  it("fetches updated draft and updates form when EventSource emits a higher version put event", async () => {
    const initialManifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "shared-source", namespace: "banskabystrica" },
      spec: { type: "http", http: { url: "https://initial.example.com" } },
    };

    const updatedManifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "shared-source", namespace: "banskabystrica" },
      spec: {
        type: "http",
        http: { url: "https://updated-from-other-window.example.com" },
      },
    };

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), {
          status: 200,
        });
      }
      if (
        url.includes(
          "/api/v1/projects/banskabystrica/drafts/DataSource/shared-source",
        )
      ) {
        // If event with version 2 was emitted, return updated draft
        const v2 = MockEventSource.instances.some(
          (inst) => (inst as unknown as { triggeredV2?: boolean }).triggeredV2,
        );
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "shared-source",
            manifest: v2 ? updatedManifest : initialManifest,
            verdict: null,
            touchedBy: v2 ? "jana.kovacova" : "demo.steward",
            touchedKind: "person",
            version: v2 ? 2 : 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Edit Data Source"
            description="Edit draft"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="shared-source"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    // Verify initial load
    const urlInput = await screen.findByLabelText(/URL/i);
    await waitFor(() => {
      expect(urlInput).toHaveValue("https://initial.example.com");
    });

    const statusEl = screen.getByTestId("draft-status");
    expect(statusEl).toHaveTextContent(
      "Draft v1, last touched by demo.steward (person)",
    );

    // Simulate EventSource receiving a put event for version 2 from another client
    expect(MockEventSource.instances.length).toBeGreaterThan(0);
    const eventSource = MockEventSource.instances[0];
    (eventSource as unknown as { triggeredV2: boolean }).triggeredV2 = true;

    await act(async () => {
      eventSource.emit("draft", {
        project: "banskabystrica",
        kind: "DataSource",
        name: "shared-source",
        version: 2,
        touchedBy: "jana.kovacova",
        touchedKind: "person",
        event: "put",
        updatedAt: "2026-09-13T12:05:00Z",
      });
    });

    // Verify form reloaded with updated values from window B
    await waitFor(() => {
      expect(urlInput).toHaveValue(
        "https://updated-from-other-window.example.com",
      );
    });

    expect(screen.getByTestId("draft-status")).toHaveTextContent(
      "Draft v2, last touched by jana.kovacova (person)",
    );
  });

  it("disables Propose with drafts.proposeReason.none when no verdict is present", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), {
          status: 200,
        });
      }
      if (
        url.includes(
          "/api/v1/projects/banskabystrica/drafts/DataSource/unverified",
        )
      ) {
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "unverified",
            manifest: {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "DataSource",
              metadata: { name: "unverified", namespace: "banskabystrica" },
              spec: { type: "http", http: { url: "https://api.example.com" } },
            },
            verdict: null,
            touchedBy: "demo.steward",
            touchedKind: "person",
            version: 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Unchecked Draft"
            description="Gate test"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="unverified"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    await waitFor(() => {
      expect(submitBtn).toBeDisabled();
    });

    expect(screen.getByTestId("draft-verdict")).toHaveTextContent(
      en.drafts.verdict.none,
    );
    expect(screen.getByTestId("propose-reason")).toHaveTextContent(
      en.drafts.proposeReason.none,
    );
  });

  it("says the check is missing before the first draft save, and the check enables Propose (T-0779)", async () => {
    const proposed: unknown[] = [];
    const checks: unknown[] = [];
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : input.toString();
        const method =
          input instanceof Request ? input.method : (init?.method ?? "GET");
        if (url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), {
            status: 200,
          });
        }
        // The Check is the kind's dry run; it answers a verdict fresh for what it judged.
        if (method === "POST" && url.includes("dryRun=All")) {
          const body = JSON.parse(await (input as Request).clone().text());
          checks.push(body);
          const manifest = { ...(body as Record<string, unknown>) };
          delete manifest.draft;
          return new Response(
            JSON.stringify({
              verdict: {
                ok: true,
                findings: [],
                checkedAt: new Date().toISOString(),
                inputDigest: digestOf(manifest),
              },
            }),
            { status: 200 },
          );
        }
        if (method === "PUT" && url.includes("/drafts/DataSource/aq-feed")) {
          const body = JSON.parse(await (input as Request).clone().text());
          return new Response(
            JSON.stringify({
              project: "banskabystrica",
              kind: "DataSource",
              name: "aq-feed",
              manifest: body.manifest,
              verdict: null,
              touchedBy: "demo.steward",
              touchedKind: "person",
              version: 1,
              updatedAt: "2026-09-13T12:00:00Z",
            }),
            { status: 200 },
          );
        }
        return new Response(JSON.stringify({}), { status: 200 });
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Create Data Source"
            description="Create draft"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={(form) => proposed.push(form)}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    fireEvent.change(screen.getByLabelText(/Name/i), {
      target: { value: "aq-feed" },
    });

    // No draft has been saved yet, and the button already says why it proposes nothing.
    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    await waitFor(() => {
      expect(submitBtn).toBeDisabled();
    });
    expect(screen.getByTestId("propose-reason")).toHaveTextContent(
      en.drafts.proposeReason.none,
    );

    fireEvent.click(screen.getByRole("button", { name: en.form.check }));
    await waitFor(() => {
      expect(submitBtn).toBeEnabled();
    });
    expect(checks).toHaveLength(1);
    expect(screen.queryByTestId("propose-reason")).toBeNull();

    fireEvent.click(submitBtn);
    await waitFor(() => {
      expect(proposed).toHaveLength(1);
    });
  });

  it("warns that applying restarts the stream when the check says so (T-1056)", async () => {
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : input.toString();
        const method =
          input instanceof Request ? input.method : (init?.method ?? "GET");
        if (url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), {
            status: 200,
          });
        }
        // The dry run answers the restart along with the verdict: the runner drops and
        // recreates the stream for a spec change, and a periodic pipeline loses its schedule.
        if (method === "POST" && url.includes("dryRun=All")) {
          const body = JSON.parse(await (input as Request).clone().text());
          const manifest = { ...(body as Record<string, unknown>) };
          delete manifest.draft;
          return new Response(
            JSON.stringify({
              restartsStream: true,
              verdict: {
                ok: true,
                findings: [],
                checkedAt: new Date().toISOString(),
                inputDigest: digestOf(manifest),
              },
            }),
            { status: 200 },
          );
        }
        return new Response(JSON.stringify({}), { status: 200 });
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Create Data Source"
            description="Create draft"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    fireEvent.change(screen.getByLabelText(/Name/i), {
      target: { value: "aq-feed" },
    });

    // Nothing is claimed before the check has answered.
    expect(screen.queryByText(en.form.restartsStream)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: en.form.check }));
    expect(await screen.findByText(en.form.restartsStream)).toBeInTheDocument();
  });

  it("leaves Propose refused when the check itself is refused (T-0779)", async () => {
    const proposed: unknown[] = [];
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : input.toString();
        const method =
          input instanceof Request ? input.method : (init?.method ?? "GET");
        if (url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), {
            status: 200,
          });
        }
        if (method === "POST" && url.includes("dryRun=All")) {
          // The role may edit its draft and not propose the kind: the check is refused.
          return new Response(
            JSON.stringify({ title: "Forbidden", detail: "not yours" }),
            { status: 403, headers: { "content-type": "application/problem+json" } },
          );
        }
        return new Response(JSON.stringify({}), { status: 200 });
      },
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Create Data Source"
            description="Create draft"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={(form) => proposed.push(form)}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    fireEvent.change(screen.getByLabelText(/Name/i), {
      target: { value: "refused-feed" },
    });
    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    fireEvent.click(screen.getByRole("button", { name: en.form.check }));

    await waitFor(() => {
      expect(screen.getByTestId("draft-verdict")).toHaveTextContent(
        en.drafts.verdict.none,
      );
    });
    expect(submitBtn).toBeDisabled();
    expect(screen.getByTestId("propose-reason")).toHaveTextContent(
      en.drafts.proposeReason.none,
    );
    expect(proposed).toHaveLength(0);
  });

  it("offers no check to a dialog that has no collection to check against", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), {
          status: 200,
        });
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Create Space"
            description="A form that names no draft"
            project="banskabystrica"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await screen.findByRole("button", { name: "Propose change" });
    expect(screen.queryByRole("button", { name: en.form.check })).toBeNull();
  });

  it("enables Propose when a fresh green verdict matches the draft digest", async () => {
    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "verified", namespace: "banskabystrica" },
      spec: { type: "http", http: { url: "https://api.example.com" } },
    };
    const digest = digestOf(manifest);

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), {
          status: 200,
        });
      }
      if (
        url.includes(
          "/api/v1/projects/banskabystrica/drafts/DataSource/verified",
        )
      ) {
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "verified",
            manifest,
            verdict: {
              ok: true,
              findings: [],
              checkedAt: "2026-09-13T12:00:00Z",
              inputDigest: digest,
            },
            touchedBy: "demo.steward",
            touchedKind: "person",
            version: 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Verified Draft"
            description="Gate test"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="verified"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    await waitFor(() => {
      expect(submitBtn).toBeEnabled();
    });

    const verdictEl = screen.getByTestId("draft-verdict");
    expect(verdictEl).toHaveTextContent("Checked");
    expect(screen.queryByTestId("propose-reason")).toBeNull();
  });

  it("saves the draft the form shows before Propose names it, when the debounce has not sent it yet (T-0769)", async () => {
    const manifestOf = (url: string) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "handed-off", namespace: "banskabystrica" },
      spec: { type: "http", http: { url } },
    });
    const shown = manifestOf("https://checked.example.com");
    const order: string[] = [];

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : input.toString();
      const method = input instanceof Request ? input.method : (init?.method ?? "GET");
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), { status: 200 });
      }
      if (url.includes("/api/v1/projects/banskabystrica/drafts/DataSource/handed-off")) {
        const put = method === "PUT";
        const body = put ? JSON.parse(await (input as Request).clone().text()) : undefined;
        if (put) {
          order.push(`put ${body.manifest.spec.http.url}`);
        }
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "handed-off",
            manifest: put ? body.manifest : manifestOf("https://typed.example.com"),
            // The check ran on what the form shows; the save of it had not landed.
            verdict: {
              ok: true,
              findings: [],
              checkedAt: "2026-09-13T12:00:00Z",
              inputDigest: digestOf(shown),
            },
            touchedBy: "demo.steward",
            touchedKind: "person",
            version: put ? 2 : 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);
    const onSubmit = vi.fn((_form: TestFormData, draft?: { kind: string; name: string }) => {
      order.push(`propose ${draft?.kind}/${draft?.name}`);
    });

    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Handed-off Draft"
            description="Flush before propose"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="handed-off"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={onSubmit}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", { name: "Propose change" });
    await waitFor(() => expect(submitBtn).toBeDisabled());
    fireEvent.change(screen.getByLabelText(/URL/i), {
      target: { value: "https://checked.example.com" },
    });
    await waitFor(() => expect(submitBtn).toBeEnabled());
    fireEvent.click(submitBtn);

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(order).toEqual(["put https://checked.example.com", "propose DataSource/handed-off"]);
    // The debounced save finds the manifest written and sends nothing more.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });
    expect(order).toHaveLength(2);
  });

  it("proposes nothing when another window changed the draft Propose would save (T-0769)", async () => {
    const manifestOf = (url: string) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "contested", namespace: "banskabystrica" },
      spec: { type: "http", http: { url } },
    });
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      if (request.url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), { status: 200 });
      }
      if (request.method === "PUT") {
        return new Response(JSON.stringify({ current: 3 }), { status: 409 });
      }
      return new Response(
        JSON.stringify({
          project: "banskabystrica",
          kind: "DataSource",
          name: "contested",
          manifest: manifestOf("https://typed.example.com"),
          verdict: { ok: true, findings: [], checkedAt: "2026-09-13T12:00:00Z", inputDigest: digestOf(manifestOf("https://checked.example.com")) },
          touchedBy: "demo.steward",
          touchedKind: "person",
          version: 2,
          updatedAt: "2026-09-13T12:00:00Z",
        }),
        { status: 200 },
      );
    });
    vi.stubGlobal("fetch", fetchMock);
    const onSubmit = vi.fn();

    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Contested Draft"
            description="Conflict before propose"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="contested"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={onSubmit}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", { name: "Propose change" });
    await waitFor(() => expect(submitBtn).toBeDisabled());
    fireEvent.change(screen.getByLabelText(/URL/i), { target: { value: "https://checked.example.com" } });
    await waitFor(() => expect(submitBtn).toBeEnabled());
    fireEvent.click(submitBtn);

    expect(await screen.findByText(en.drafts.conflict)).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("marks verdict as stale and disables Propose when form is changed after check", async () => {
    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "verified-then-changed", namespace: "banskabystrica" },
      spec: { type: "http", http: { url: "https://api.example.com" } },
    };
    const digest = digestOf(manifest);

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), {
          status: 200,
        });
      }
      if (
        url.includes(
          "/api/v1/projects/banskabystrica/drafts/DataSource/verified-then-changed",
        )
      ) {
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "verified-then-changed",
            manifest,
            verdict: {
              ok: true,
              findings: [],
              checkedAt: "2026-09-13T12:00:00Z",
              inputDigest: digest,
            },
            touchedBy: "demo.steward",
            touchedKind: "person",
            version: 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Modified Draft"
            description="Stale verdict test"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="verified-then-changed"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    await waitFor(() => {
      expect(submitBtn).toBeEnabled();
    });

    // Modify the URL field
    const urlInput = screen.getByLabelText(/URL/i);
    fireEvent.change(urlInput, {
      target: { value: "https://modified.example.com" },
    });

    // The digest now diverges from inputDigest -> verdict becomes stale
    await waitFor(() => {
      expect(submitBtn).toBeDisabled();
    });

    expect(screen.getByTestId("draft-verdict")).toHaveTextContent(
      en.drafts.verdict.stale,
    );
    expect(screen.getByTestId("propose-reason")).toHaveTextContent(
      en.drafts.proposeReason.stale,
    );
  });

  it("enables Propose with warning when validation is lax", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "lax" }), {
          status: 200,
        });
      }
      if (
        url.includes(
          "/api/v1/projects/banskabystrica/drafts/DataSource/lax-test",
        )
      ) {
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "lax-test",
            manifest: {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "DataSource",
              metadata: { name: "lax-test", namespace: "banskabystrica" },
              spec: { type: "http", http: { url: "https://api.example.com" } },
            },
            verdict: null,
            touchedBy: "demo.steward",
            touchedKind: "person",
            version: 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <BrandingProvider>
            <Harness
              open={true}
              onOpenChange={() => {}}
              title="Lax Draft"
              description="Lax validation test"
              project="banskabystrica"
              draftKind="DataSource"
              plural="datasources"
              draftName="lax-test"
              schema={TEST_SCHEMA}
              submitLabel="Propose change"
              source={TEST_SOURCE}
              onSubmit={() => {}}
            />
          </BrandingProvider>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    // In lax mode, button stays enabled despite absent verdict
    await waitFor(() => {
      expect(submitBtn).toBeEnabled();
    });

    expect(screen.getByText(en.drafts.laxWarning)).toBeInTheDocument();
  });

  it("renders verdict findings when check reports errors", async () => {
    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "with-findings", namespace: "banskabystrica" },
      spec: { type: "http", http: { url: "not-a-valid-url" } },
    };
    const digest = digestOf(manifest);

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url.includes("/api/v1/branding")) {
        return new Response(JSON.stringify({ validation: "strict" }), {
          status: 200,
        });
      }
      if (
        url.includes(
          "/api/v1/projects/banskabystrica/drafts/DataSource/with-findings",
        )
      ) {
        return new Response(
          JSON.stringify({
            project: "banskabystrica",
            kind: "DataSource",
            name: "with-findings",
            manifest,
            verdict: {
              ok: false,
              findings: [
                {
                  level: "error",
                  path: "spec.http.url",
                  message: "must be a valid http or https URL",
                },
              ],
              checkedAt: "2026-09-13T12:00:00Z",
              inputDigest: digest,
            },
            touchedBy: "demo.steward",
            touchedKind: "person",
            version: 1,
            updatedAt: "2026-09-13T12:00:00Z",
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({}), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness
            open={true}
            onOpenChange={() => {}}
            title="Failed Check Draft"
            description="Findings test"
            project="banskabystrica"
            draftKind="DataSource"
            plural="datasources"
            draftName="with-findings"
            schema={TEST_SCHEMA}
            submitLabel="Propose change"
            source={TEST_SOURCE}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const submitBtn = await screen.findByRole("button", {
      name: "Propose change",
    });
    await waitFor(() => {
      expect(submitBtn).toBeDisabled();
    });

    const findingsEl = screen.getByTestId("draft-findings");
    expect(findingsEl).toBeInTheDocument();
    expect(findingsEl).toHaveTextContent("spec.http.url");
    expect(findingsEl).toHaveTextContent("must be a valid http or https URL");

    expect(screen.getByTestId("propose-reason")).toHaveTextContent(
      en.drafts.proposeReason.red,
    );
  });
});
