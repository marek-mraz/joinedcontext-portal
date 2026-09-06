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
  roles: ["portal-editor"],
};

const ACCOUNTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ServiceAccount",
      metadata: { name: "vendorx-parking-push", namespace: "banskabystrica" },
      spec: {
        owner: { user: "jana.kovacova" },
        purpose: "VendorX pushes ParkingSpot updates every 10 s",
        roles: [{ role: "space-writer", scope: { contextSpace: "parking" } }],
        credentials: [
          { kind: "oauth-client", name: "main" },
          { kind: "api-key", name: "legacy-push" },
        ],
      },
    },
  ],
};

const KEYS = {
  items: [
    {
      keyId: "3f9c2a7b1d4e8f06",
      credential: "legacy-push",
      createdAt: "2026-09-06T18:20:11Z",
      createdBy: "jana.kovacova",
      expiresAt: "2027-03-01T00:00:00Z",
    },
  ],
};

const MINTED = {
  keyId: "aa11bb22cc33dd44",
  // Deliberately low entropy and self-describing: the previous fixture was a plausible
  // base64 secret and gitleaks' generic-api-key rule stopped the whole ci lane on it.
  token: "jc_aa11bb22cc33dd44_EXAMPLE_NOT_A_REAL_TOKEN",
  credential: "legacy-push",
};

function renderAccess(options: { keysStatus?: number } = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    // The generated client hands over a `Request`; the gateway call is a plain `fetch(url)`.
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const method = request?.method ?? "GET";
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(status === 204 ? null : JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/keys") && method === "GET") {
      return json(KEYS, options.keysStatus ?? 200);
    }
    if (path.includes("/keys") && method === "POST") {
      return json(MINTED, 201);
    }
    if (path.includes("/keys") && method === "DELETE") {
      return Promise.resolve(new Response(null, { status: 204 }));
    }
    if (path.endsWith("/serviceaccounts")) {
      return json(ACCOUNTS);
    }
    if (path.endsWith("/access")) {
      return json({ permissions: [], prohibitions: [] });
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

describe("service accounts view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=csrf-token-value";
    window.history.pushState({}, "", "/projects/banskabystrica/access");
    Object.assign(navigator, { clipboard: { writeText: vi.fn(() => Promise.resolve()) } });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the account, its owner and the keys behind its api-key credential", async () => {
    renderAccess();

    expect(await screen.findByText("vendorx-parking-push")).toBeInTheDocument();
    expect(screen.getByText(/VendorX pushes ParkingSpot updates/)).toBeInTheDocument();
    expect(await screen.findByText("3f9c2a7b1d4e8f06")).toBeInTheDocument();
    // The oauth-client credential is provisioned in Keycloak and has no key button here.
    expect(
      screen.getByRole("button", { name: /New API key \(legacy-push\)/ }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /New API key \(main\)/ })).not.toBeInTheDocument();
  });

  it("shows a minted key once, in a dialog that warns it will not be shown again (PF-36)", async () => {
    renderAccess();

    await userEvent.click(
      await screen.findByRole("button", { name: /New API key \(legacy-push\)/ }),
    );

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(en.access.keys.onceWarning)).toBeInTheDocument();
    const field = within(dialog).getByLabelText(en.access.keys.token) as HTMLInputElement;
    expect(field.value).toBe(MINTED.token);

    await userEvent.click(within(dialog).getByRole("button", { name: en.access.keys.copy }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(MINTED.token);

    await userEvent.click(within(dialog).getByRole("button", { name: en.access.keys.done }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // Gone for good: the listing the view falls back to has no token in it.
    expect(screen.queryByText(MINTED.token)).not.toBeInTheDocument();
  });

  it("mints against the account in the path and never sends the token back", async () => {
    const fetchMock = renderAccess();

    await userEvent.click(
      await screen.findByRole("button", { name: /New API key \(legacy-push\)/ }),
    );

    const write = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.method === "POST");
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(new URL(write.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/serviceaccounts/vendorx-parking-push/keys",
    );
    expect(write.headers.get("x-csrf-token")).toBe("csrf-token-value");
    expect(JSON.parse(await write.clone().text())).toEqual({ credential: "legacy-push" });
  });

  it("asks before revoking a key and only then calls DELETE", async () => {
    const fetchMock = renderAccess();

    await userEvent.click(await screen.findByRole("button", { name: en.access.keys.revoke }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent("3f9c2a7b1d4e8f06");
    expect(
      fetchMock.mock.calls.some((call) => (call[0] as Request).method === "DELETE"),
    ).toBe(false);

    await userEvent.click(
      within(confirm).getByRole("button", { name: en.access.keys.revokeNow }),
    );
    const deleted = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.method === "DELETE");
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(new URL(deleted.url).pathname).toMatch(/\/keys\/3f9c2a7b1d4e8f06$/);
  });

  it("rotates through the key's own route", async () => {
    const fetchMock = renderAccess();

    await userEvent.click(await screen.findByRole("button", { name: en.access.keys.rotate }));

    const rotated = await waitFor(() => {
      const request = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((candidate) => candidate.url.includes("/rotate"));
      expect(request).toBeDefined();
      return request as Request;
    });
    expect(rotated.method).toBe("POST");
    expect(await screen.findByRole("dialog")).toHaveTextContent(en.access.keys.onceWarning);
  });

  it("says the key store is missing rather than showing an empty list", async () => {
    renderAccess({ keysStatus: 503 });

    expect(await screen.findByText(en.access.keys.noStore)).toBeInTheDocument();
    expect(screen.queryByText(en.access.keys.empty)).not.toBeInTheDocument();
  });
});
