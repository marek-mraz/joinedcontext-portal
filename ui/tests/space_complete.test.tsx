import { StrictMode } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi, beforeEach } from "vitest";
import i18n from "../src/i18n";
import { SpaceComplete } from "../src/pages/spaces/SpaceComplete";
import { rememberPrefill, takePrefill } from "../src/assistant/state";
import en from "../src/locales/en.json";

const navigate = vi.fn();

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => navigate,
  Link: ({ children, to }: { children: React.ReactNode; to: string }) => <a href={to}>{children}</a>,
}));

function renderComponent(strict = false) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const page = (
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <SpaceComplete project="helsinki" />
      </I18nextProvider>
    </QueryClientProvider>
  );
  return render(strict ? <StrictMode>{page}</StrictMode> : page);
}

describe("SpaceComplete page", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("renders inputs and runs completion when Complete is clicked", async () => {
    const user = userEvent.setup();

    const mockResponse = {
      space: "bikes",
      found: [],
      drafts: [
        {
          kind: "DataModel",
          name: "bikes",
          inferred: true,
          manifest: { kind: "DataModel", metadata: { name: "bikes" } },
          verdict: { ok: true, findings: [], inputDigest: "abc" },
        },
        {
          kind: "DataSource",
          name: "bikes-source",
          inferred: true,
          manifest: { kind: "DataSource", metadata: { name: "bikes-source" } },
          verdict: { ok: true, findings: [], inputDigest: "def" },
        },
      ],
      proposeReady: true,
      lane: "yellow",
      change: null,
    };

    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(mockResponse), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    renderComponent();

    expect(screen.getByLabelText(/Endpoint URL/i)).toBeInTheDocument();
    const urlInput = screen.getByLabelText(/Endpoint URL/i);
    await user.type(urlInput, "https://example.com/station_status.json");

    const completeBtn = screen.getByRole("button", { name: "Complete" });
    expect(completeBtn).toBeEnabled();
    await user.click(completeBtn);

    await waitFor(() => {
      expect(screen.getByTestId("complete-draft-DataModel")).toBeInTheDocument();
    });
    expect(screen.getByTestId("complete-draft-DataSource")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Propose all/i })).toBeInTheDocument();
  });

  it("reads each draft without kind names: a label, what it is, whether it is new and whether its check passed", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    const draft = (kind: string, name: string, spec: Record<string, unknown>, ok = true, inferred = true) => ({
      kind,
      name,
      inferred,
      manifest: { kind, metadata: { name }, spec },
      verdict: { ok, findings: ok ? [] : [{ level: "error", path: "spec.http.url", message: "the feed answered 404 Not Found" }], inputDigest: "abc" },
    });
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://gbfs.example.org/station_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          draft("DataModel", "city-bikes", {
            classes: ["BikeStation"],
            // Model Tools lists a class's slots; a hand-written model may declare attributes inline.
            source: "classes:\n  BikeStation:\n    slots: [name]\n    attributes:\n      capacity: {}\n",
          }),
          draft("ContextSpace", "city-bikes", { dataModelRef: { kind: "DataModel", name: "city-bikes" } }, true, false),
          draft("DataSource", "city-bikes-source", { type: "http", http: { url: "https://gbfs.example.org/station_status.json" } }, false),
          draft("Pipeline", "city-bikes-load", { period: "60s", output: { type: "BikeStation" } }),
          draft("Endpoint", "city-bikes-all", { audience: "organization" }),
          draft("Layer", "city-bikes-map", { sourceEndpointRef: "city-bikes-all", entityType: "BikeStation", style: "circle", colorBy: { property: "capacity" } }),
          draft("Dashboard", "city-bikes", { title: "City Bikes map", visibility: "project", pages: [{ layout: "full-map", layers: ["city-bikes-map"] }] }),
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });

    renderComponent();

    const model = await screen.findByTestId("complete-draft-DataModel");
    expect(model).toHaveTextContent("Data model");
    expect(model).toHaveTextContent("Type BikeStation with 2 attributes");
    expect(model).toHaveTextContent("Drafted");
    expect(model).toHaveTextContent("Check passed");
    expect(model).not.toHaveTextContent("DataModel");
    expect(screen.getByTestId("complete-draft-ContextSpace")).toHaveTextContent("Already there");
    const source = screen.getByTestId("complete-draft-DataSource");
    expect(source).toHaveTextContent("Fetches gbfs.example.org");
    expect(source).toHaveTextContent("Check failed");
    expect(source).toHaveTextContent("spec.http.url: the feed answered 404 Not Found");
    expect(screen.getByTestId("complete-draft-Pipeline")).toHaveTextContent("Loads BikeStation every 60s");
    expect(screen.getByTestId("complete-draft-Endpoint")).toHaveTextContent("Readable by every project of the organization");
    // The map over the new endpoint is drafted with the rest (AG-79).
    expect(screen.getByTestId("complete-draft-Layer")).toHaveTextContent("Map layer");
    expect(screen.getByTestId("complete-draft-Layer")).toHaveTextContent("Draws BikeStation on the map, coloured by capacity");
    expect(screen.getByTestId("complete-draft-Dashboard")).toHaveTextContent("A map dashboard with 1 page");
    expect(screen.queryByText(/\b(green|red|Inferred)\b/)).not.toBeInTheDocument();
    // Proposing is the next step, so it is the primary action and says what approval does.
    expect(screen.getByRole("button", { name: /Propose all/i }).className).toContain("bg-primary");
    expect(screen.getByRole("button", { name: "Complete" }).className).not.toContain("bg-primary");
    expect(screen.getByText(/Once a person approves them/)).toBeInTheDocument();
    expect(screen.getByLabelText(/Space name/i)).toHaveAttribute("placeholder", "city-bikes");
  });

  it("opens with the drafts the assistant completed, ready to propose, without running again (AG-73)", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://example.com/free_bike_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          {
            kind: "Pipeline",
            name: "city-bikes-load",
            inferred: true,
            manifest: { kind: "Pipeline", metadata: { name: "city-bikes-load" } },
            verdict: { ok: true, findings: [], inputDigest: "abc" },
          },
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });

    renderComponent();

    expect(await screen.findByTestId("complete-draft-Pipeline")).toBeInTheDocument();
    expect(screen.getByLabelText(/Endpoint URL/i)).toHaveValue("https://example.com/free_bike_status.json");
    expect(screen.getByRole("button", { name: /Propose all/i })).toBeEnabled();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("keeps the handed drafts when React renders the page twice before it commits (T-0894)", async () => {
    vi.stubGlobal("fetch", vi.fn());
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://example.com/free_bike_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          {
            kind: "Pipeline",
            name: "city-bikes-load",
            inferred: true,
            manifest: { kind: "Pipeline", metadata: { name: "city-bikes-load" } },
            verdict: { ok: true, findings: [], inputDigest: "abc" },
          },
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });

    // StrictMode renders and mounts twice, as a navigation the dock's own updates interrupt does.
    renderComponent(true);

    expect(await screen.findByTestId("complete-draft-Pipeline")).toBeInTheDocument();
    expect(screen.getByLabelText(/Endpoint URL/i)).toHaveValue("https://example.com/free_bike_status.json");
  });

  it("opens each draft on its kind's page with the draft in hand (T-0771)", async () => {
    const user = userEvent.setup();
    vi.stubGlobal("fetch", vi.fn());
    navigate.mockClear();
    const source = "id: https://example.com/bikes\nname: bikes\n";
    const draft = (kind: string, name: string, spec: Record<string, unknown> = {}) => ({
      kind,
      name,
      inferred: true,
      manifest: { kind, metadata: { name }, spec },
      verdict: { ok: true, findings: [], inputDigest: "abc" },
    });
    window.history.replaceState(null, "", "/projects/helsinki/spaces/complete?space=city-bikes");
    rememberPrefill("/projects/helsinki/spaces/complete?space=city-bikes", {
      url: "https://example.com/free_bike_status.json",
      result: {
        space: "city-bikes",
        found: [],
        drafts: [
          draft("DataModel", "city-bikes", { source }),
          draft("DataSource", "city-bikes-feed"),
          draft("Pipeline", "city-bikes-load"),
          draft("Endpoint", "city-bikes-public"),
        ],
        proposeReady: true,
        lane: "yellow",
        change: null,
      },
    });
    renderComponent();

    const open = async (kind: string) =>
      user.click(within(await screen.findByTestId(`complete-draft-${kind}`)).getByRole("button", { name: en.spaces.complete.open }));

    await open("DataModel");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/models" });
    expect(takePrefill("/projects/helsinki/models")).toEqual({ source });
    await open("DataSource");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/datasources?draft=city-bikes-feed" });
    await open("Pipeline");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/pipelines?draft=city-bikes-load" });
    await open("Endpoint");
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/endpoints?draft=city-bikes-public" });
  });
});
