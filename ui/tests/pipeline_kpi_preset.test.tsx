/**
 * T-0633: Analyse (KPI) preset in the pipeline studio.
 * An indicator is generated from an endpoint query, folded into a KeyPerformanceIndicator,
 * tested through the test route against the live endpoint URL, and proposed.
 */
import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { Manifest } from "../src/api/manifest";

function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return (
    <textarea
      aria-label="YAML"
      value={value}
      onChange={(event) => onChange?.(event.target.value)}
    />
  );
}
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: MockEditor,
}));

const { PipelineStudio } = await import("../src/pages/pipelines/PipelineStudio");

const ENDPOINTS: Manifest[] = [
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-all", namespace: "helsinki" },
    spec: { contextSpaceRef: "helsinki", slug: "abc123", audience: "public" },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "kpi-writer", namespace: "helsinki" },
    spec: { contextSpaceRef: "helsinki-kpi", slug: "kpi456" },
  },
];

const KPI_TEST_ANSWER = {
  input: { events: 1, bytes: 10 },
  mapping: [{ id: "x", type: "KeyPerformanceIndicator", currentValue: { value: 12.5 } }],
  validation: [{ index: 0, ok: true, problems: [] }],
  errors: [],
};

function mockFetch(testResponse?: { status: number; body: unknown }) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (url.includes("/pipelines/test") && method === "POST") {
      return json(testResponse?.body ?? {}, testResponse?.status ?? 200);
    }
    return json({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [],
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function Harness({
  initial,
  onChange,
  onVerdict,
  endpoints = ENDPOINTS,
}: {
  initial?: PipelineForm;
  onChange: (form: PipelineForm) => void;
  onVerdict?: (ok: boolean, bloblang: string) => void;
  endpoints?: Manifest[];
}) {
  const [draft, setDraft] = useState<PipelineForm | undefined>(initial);
  return (
    <PipelineStudio
      project="helsinki"
      draft={draft}
      onChange={(form) => {
        setDraft(form);
        onChange(form);
      }}
      dataSources={[]}
      endpoints={endpoints}
      toManifest={(form) => ({
        kind: "Pipeline",
        metadata: { name: form.name },
        spec: form,
      })}
      onVerdict={onVerdict}
    />
  );
}

function renderStudio(options?: {
  initial?: PipelineForm;
  onChange?: (form: PipelineForm) => void;
  onVerdict?: (ok: boolean, bloblang: string) => void;
  endpoints?: Manifest[];
}) {
  const onChange = options?.onChange ?? vi.fn();
  const onVerdict = options?.onVerdict ?? vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Harness
          initial={options?.initial}
          onChange={onChange}
          onVerdict={onVerdict}
          endpoints={options?.endpoints ?? ENDPOINTS}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { onChange, onVerdict };
}

describe("PipelineStudio KPI preset", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("choosing preset kpi and endpoint helsinki-all calls onChange with a KPI form", async () => {
    mockFetch();
    const { onChange } = renderStudio();

    const presetSelect = screen.getByLabelText(en.pipelines.studio.preset.title);
    await userEvent.selectOptions(presetSelect, "kpi");

    const endpointSelect = await screen.findByLabelText(en.pipelines.studio.kpi.endpoint);
    await userEvent.selectOptions(endpointSelect, "helsinki-all");

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const lastForm = (onChange as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0] as PipelineForm;
    expect((lastForm.source?.endpointRef as unknown as { name: string })?.name).toBe(
      "helsinki-all",
    );
    expect(lastForm.output?.type).toBe("KeyPerformanceIndicator");
    expect(lastForm.targetEndpoint).toContain("kpi-writer");
    expect(lastForm.compute?.bloblang).toContain("availableBikeNumber");
    expect(lastForm.compute?.bloblang).toContain('"C62"');
  });

  it("clicking the test button POSTs the sample url and shows the computed value", async () => {
    const fetchMock = mockFetch({ status: 200, body: KPI_TEST_ANSWER });
    const { onVerdict } = renderStudio();

    const presetSelect = screen.getByLabelText(en.pipelines.studio.preset.title);
    await userEvent.selectOptions(presetSelect, "kpi");
    const endpointSelect = await screen.findByLabelText(en.pipelines.studio.kpi.endpoint);
    await userEvent.selectOptions(endpointSelect, "helsinki-all");

    const testBtn = screen.getByTestId("studio-kpi-test");
    await userEvent.click(testBtn);

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([input]) =>
        String(input).includes("/pipelines/test"),
      );
      expect(call).toBeDefined();
    });

    const call = fetchMock.mock.calls.find(([input]) => String(input).includes("/pipelines/test"))!;
    const requestInit = call[1] as RequestInit;
    const body = JSON.parse(requestInit.body as string) as {
      sample: { url: string; format: string };
      pipeline: { spec: PipelineForm };
    };
    expect(body.sample.url).toContain(
      "/api/endpoint/abc123/ngsi-ld/v1/entities?type=BikeHireDockingStation",
    );
    expect(body.sample.format).toBe("json");

    const valueEl = await screen.findByTestId("studio-kpi-value");
    expect(valueEl).toHaveTextContent("12.5");
    expect(onVerdict).toHaveBeenCalledWith(true, expect.stringContaining("availableBikeNumber"));
  });

  it("shows an alert when the pipeline test endpoint fails", async () => {
    mockFetch({
      status: 500,
      body: { detail: "Failed to fetch from endpoint" },
    });
    renderStudio();

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(
      await screen.findByLabelText(en.pipelines.studio.kpi.endpoint),
      "helsinki-all",
    );

    await userEvent.click(screen.getByTestId("studio-kpi-test"));

    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to fetch from endpoint");
  });

  it("updates Bloblang and calls onChange when attribute or aggregate change", async () => {
    mockFetch();
    const { onChange } = renderStudio();

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(
      await screen.findByLabelText(en.pipelines.studio.kpi.endpoint),
      "helsinki-all",
    );

    const attrInput = screen.getByLabelText(en.pipelines.studio.kpi.attribute);
    await userEvent.clear(attrInput);
    await userEvent.type(attrInput, "freeBikeCount");

    await waitFor(() => {
      const form = (onChange as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0] as PipelineForm;
      expect(form.compute?.bloblang).toContain("freeBikeCount");
    });
  });
});
