import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi, beforeEach } from "vitest";
import i18n from "../src/i18n";
import { SpaceComplete } from "../src/pages/spaces/SpaceComplete";

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => vi.fn(),
  Link: ({ children, to }: { children: React.ReactNode; to: string }) => <a href={to}>{children}</a>,
}));

function renderComponent() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <SpaceComplete project="helsinki" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
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
});
