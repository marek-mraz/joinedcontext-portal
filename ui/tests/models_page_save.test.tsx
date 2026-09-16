import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import i18n from "../src/i18n";
import { queryKeys } from "../src/api/client";
import { rememberPrefill } from "../src/assistant/state";
import en from "../src/locales/en.json";
import { ModelsPage } from "../src/pages/models/ModelsPage";

const PUBLISHED_LINKML = `id: https://example.org/models/air-quality
name: air-quality
prefixes:
  aq: https://example.org/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: https://example.org/aq/AirQualityObserved
    slots:
      - dateObserved
slots:
  dateObserved:
    range: string
    required: true
`;

function renderWithClient(
  ui: React.ReactElement,
  queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  }),
) {
  const rootRoute = createRootRoute();
  const home = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>,
  });
  const approvals = createRoute({
    getParentRoute: () => rootRoute,
    path: "/projects/$project/approvals/$id",
    component: () => <p>approval page</p>,
  });
  const router = createRouter({ routeTree: rootRoute.addChildren([home, approvals]) });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}

describe("ModelsPage save and source loading (DM-56)", () => {
  let originalFetch: typeof global.fetch;

  beforeEach(async () => {
    originalFetch = global.fetch;
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("opening a published model calls GET .../source and the editor shows it", async () => {
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      if (urlStr.includes("/datamodels/air-quality/source")) {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{
          name: "air-quality",
          version: "1.0.0",
          lifecycle: "published",
        }}
      />,
    );

    // Verify GET source called
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        expect.stringContaining("/api/v1/projects/ovzdusie/datamodels/air-quality/source"),
        expect.anything(),
      );
    });

    // Check that editor view loaded the classes
    await waitFor(() => {
      expect(screen.getAllByText("AirQualityObserved").length).toBeGreaterThan(0);
    });
  });

  it("opens when another page already cached the spaces and endpoints lists as the API answers them", async () => {
    const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    const space = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "air", namespace: "ovzdusie" },
      spec: { dataModelRef: "air-quality" },
    };
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      if (urlStr.includes("/datamodels/air-quality/source")) {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/spaces")) {
        return Promise.resolve(new Response(JSON.stringify(list([space])), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify(list([])), { status: 200 }));
    });
    global.fetch = fetchMock;

    // The keys are shared: the Spaces or Endpoints page, or the assistant, filled them first.
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    queryClient.setQueryData(queryKeys.list("ovzdusie", "spaces"), list([space]));
    queryClient.setQueryData(queryKeys.list("ovzdusie", "endpoints"), list([]));

    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{ name: "air-quality", version: "1.0.0", lifecycle: "published" }}
      />,
      queryClient,
    );

    await waitFor(() => {
      expect(screen.getAllByText("AirQualityObserved").length).toBeGreaterThan(0);
    });
    expect(queryClient.getQueryData(queryKeys.list("ovzdusie", "spaces"))).toMatchObject({
      kind: "List",
    });
  });

  it("Save calls PUT with text body and Change notice appears on 202", async () => {
    const user = userEvent.setup();
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");

      if (urlStr.includes("/datamodels/air-quality/source") && method === "GET") {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/datamodels/air-quality/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Change",
              metadata: { name: "mr-88", namespace: "ovzdusie" },
              status: {
                phase: "PendingApproval",
                lane: "yellow",
                mergeRequest: "https://forge.example.sk/pulls/88",
                plan: { create: 0, update: 6, delete: 0 },
              },
            }),
            { status: 202, headers: { "Content-Type": "application/json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{
          name: "air-quality",
          version: "1.0.0",
          lifecycle: "published",
        }}
      />,
    );

    const saveBtn = await screen.findByRole("button", { name: /save model/i });
    await user.click(saveBtn);

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        expect.stringContaining("/api/v1/projects/ovzdusie/datamodels/air-quality/source"),
        expect.objectContaining({
          method: "PUT",
        }),
      );
    });

    // Check notice rendered
    await waitFor(() => {
      expect(screen.getByText(/mr-88/i)).toBeInTheDocument();
    });
  });

  it("Save shows problem detail on 400 response", async () => {
    const user = userEvent.setup();
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");

      if (urlStr.includes("/datamodels/air-quality/source") && method === "GET") {
        return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
      }
      if (urlStr.includes("/datamodels/air-quality/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              type: "https://joinedcontext.com/errors/invalid-request",
              title: "Invalid Request",
              status: 400,
              detail: "a breaking change cannot be saved under version 1.0.0; publish it as 2.0.0",
              errors: ["slot 'pm10' was removed"],
            }),
            { status: 400, headers: { "Content-Type": "application/problem+json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces") || urlStr.includes("/endpoints")) {
        return Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200 }));
      }
      return Promise.resolve(new Response("{}", { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(
      <ModelsPage
        project="ovzdusie"
        baseline={{
          name: "air-quality",
          version: "1.0.0",
          lifecycle: "published",
        }}
      />,
    );

    const saveBtn = await screen.findByRole("button", { name: /save model/i });
    await user.click(saveBtn);

    await waitFor(() => {
      expect(
        screen.getByText(/a breaking change cannot be saved under version 1.0.0/i),
      ).toBeInTheDocument();
    });
  });

  describe("a model the assistant changed (T-0738, AG-77)", () => {
    afterEach(() => {
      window.history.pushState({}, "", "/");
    });

    /** The published model, its manifest in the list, and every PUT the page sends. */
    function forge() {
      const puts: string[] = [];
      global.fetch = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
        const method = init?.method ?? (req instanceof Request ? req.method : "GET");
        if (url.includes("/datamodels/air-quality/source") && method === "PUT") {
          puts.push(String(init?.body));
          return Promise.resolve(new Response(JSON.stringify({ kind: "Change", metadata: { name: "mr-9" } }), { status: 202 }));
        }
        if (url.includes("/datamodels/air-quality/source")) {
          return Promise.resolve(new Response(PUBLISHED_LINKML, { status: 200 }));
        }
        if (url.includes("/datamodels")) {
          const model = {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "DataModel",
            metadata: { name: "air-quality", namespace: "ovzdusie" },
            spec: { version: "1.0.0", lifecycle: "published" },
          };
          return Promise.resolve(new Response(JSON.stringify({ kind: "List", items: [model] }), { status: 200 }));
        }
        return Promise.resolve(new Response(JSON.stringify({ kind: "List", items: [] }), { status: 200 }));
      });
      return puts;
    }

    it("opens with the operations applied, and a removal saves only once the breaking change is confirmed", async () => {
      const user = userEvent.setup();
      const puts = forge();
      rememberPrefill("/", { operations: [{ op: "removeSlot", name: "dateObserved" }] });
      window.history.pushState({}, "", "/?edit=air-quality");

      renderWithClient(<ModelsPage project="ovzdusie" />);

      expect(
        await screen.findByRole("heading", { name: `${en.models.severity.breaking} · ${en.lane.red}` }),
      ).toBeInTheDocument();
      expect(screen.getByText("the slot was removed; a renamed slot is a removal and an addition", { exact: false })).toBeInTheDocument();
      const save = screen.getByRole("button", { name: en.models.source.save });
      expect(save).toBeDisabled();

      await user.click(screen.getByRole("checkbox", { name: "Save it as version 2.0.0, a breaking change" }));
      expect(save).toBeEnabled();
      await user.click(save);
      await waitFor(() => expect(puts).toHaveLength(1));
      expect(puts[0]).not.toContain("dateObserved");
      expect(puts[0]).toContain("class_uri: https://example.org/aq/AirQualityObserved");
    });

    it("says which operation the model refused, and keeps the source as it is", async () => {
      forge();
      rememberPrefill("/", { operations: [{ op: "removeSlot", name: "pm25" }] });
      window.history.pushState({}, "", "/?edit=air-quality");

      renderWithClient(<ModelsPage project="ovzdusie" />);

      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent(en.models.source.handOffRefused);
      expect(alert).toHaveTextContent("unknown slot 'pm25'");
      expect(screen.getByRole("button", { name: en.models.source.save })).toBeEnabled();
    });
  });

  it("names an inferred draft and a space, then creates the model through the same route (T-0789, DM-57)", async () => {
    const user = userEvent.setup();
    const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
    const space = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "mobility", namespace: "ovzdusie" },
      spec: {},
    };
    const taken = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "air-quality", namespace: "ovzdusie" },
      spec: { contextSpaceRef: "mobility", linkml: "./air-quality.linkml.yaml" },
    };
    const fetchMock = vi.fn().mockImplementation((req: RequestInfo | URL, init?: RequestInit) => {
      const urlStr = typeof req === "string" ? req : req instanceof Request ? req.url : req.toString();
      const method = init?.method ?? (req instanceof Request ? req.method : "GET");
      if (urlStr.includes("/datamodels/bikes/source") && method === "PUT") {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Change",
              metadata: { name: "mr-91", namespace: "ovzdusie" },
              status: {
                phase: "PendingApproval",
                lane: "green",
                mergeRequest: "https://forge.example.sk/pulls/91",
                plan: { create: 6, update: 0, delete: 0 },
              },
            }),
            { status: 202, headers: { "Content-Type": "application/json" } },
          ),
        );
      }
      if (urlStr.includes("/spaces")) {
        return Promise.resolve(new Response(JSON.stringify(list([space])), { status: 200 }));
      }
      if (urlStr.includes("/datamodels")) {
        return Promise.resolve(new Response(JSON.stringify(list([taken])), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify(list([])), { status: 200 }));
    });
    global.fetch = fetchMock;

    renderWithClient(<ModelsPage project="ovzdusie" />);

    // A blank draft has no manifest, so nothing can be saved until it is named and placed.
    expect(screen.queryByRole("button", { name: /save model/i })).not.toBeInTheDocument();

    await user.click(await screen.findByRole("tab", { name: en.models.view.editor }));
    const name = await screen.findByLabelText(en.models.create.name);
    await user.type(name, "air-quality");
    await user.selectOptions(await screen.findByLabelText(en.models.create.space), "mobility");

    // A name the project already carries would fork a second model under one name.
    expect(await screen.findByRole("alert")).toHaveTextContent(/already has a model named/i);
    expect(screen.getByRole("button", { name: /save model/i })).toBeDisabled();

    await user.clear(name);
    await user.type(name, "Bikes");
    await user.click(screen.getByRole("button", { name: /save model/i }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/v1/projects/ovzdusie/datamodels/bikes/source?space=mobility",
        expect.objectContaining({ method: "PUT" }),
      );
    });
    expect(await screen.findByText(/mr-91/i)).toBeInTheDocument();
  });
});
