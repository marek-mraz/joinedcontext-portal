import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { exportUrl } from "../src/components/export/ExportModal";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica" },
      spec: { slug: "mluyob4nz52lok3ssk7pgn5vwt", audience: "public" },
    },
  ],
};

const REVISIONS = {
  items: [
    {
      sha: "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8",
      message: "Endpoint public-air: add csv",
      author: "Jana Kováčová",
      date: "2026-09-06T16:30:00Z",
    },
  ],
};

function renderEndpoints(revisionsStatus = 200) {
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
    if (path.endsWith("/revisions")) {
      return json(revisionsStatus === 200 ? REVISIONS : { status: revisionsStatus }, revisionsStatus);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
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

const downloadLink = () =>
  screen.getByRole("link", { name: en.export.download }) as HTMLAnchorElement;

describe("export modal", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("builds the download URL from the selection", () => {
    expect(exportUrl("banskabystrica", "zip", {})).toBe(
      "/api/v1/projects/banskabystrica/export?format=zip",
    );
    expect(
      exportUrl("banskabystrica", "yaml", { plural: "endpoints", name: "public-air" }, "8c56954"),
    ).toBe(
      "/api/v1/projects/banskabystrica/export?format=yaml&kinds=endpoints&names=public-air&revision=8c56954",
    );
  });

  it("offers the whole project as an archive from the shell", async () => {
    renderEndpoints();

    await userEvent.click(await screen.findByRole("button", { name: en.export.project }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(en.export.formats.zip)).toBeInTheDocument();
    expect(within(dialog).getByText(en.export.formats.yaml)).toBeInTheDocument();
    expect(within(dialog).getByText(en.export.formats.json)).toBeInTheDocument();
    // The project export defaults to the archive, which is what CC-49 promises in one click.
    expect(downloadLink()).toHaveAttribute(
      "href",
      "/api/v1/projects/banskabystrica/export?format=zip",
    );
    expect(downloadLink()).toHaveAttribute("download");
  });

  it("downloads one manifest when the export starts from its row", async () => {
    renderEndpoints();

    const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: en.export.action }));

    expect(downloadLink()).toHaveAttribute(
      "href",
      "/api/v1/projects/banskabystrica/export?format=yaml&kinds=endpoints&names=public-air",
    );

    await userEvent.click(screen.getByRole("radio", { name: /JSON list/ }));
    expect(downloadLink()).toHaveAttribute(
      "href",
      "/api/v1/projects/banskabystrica/export?format=json&kinds=endpoints&names=public-air",
    );
  });

  it("offers the history as sentences and puts the chosen revision in the URL", async () => {
    renderEndpoints();

    await userEvent.click(await screen.findByRole("button", { name: en.export.project }));
    const picker = await screen.findByRole("combobox", { name: en.export.revision });
    await screen.findByRole("option", { name: /Endpoint public-air: add csv/ });

    await userEvent.selectOptions(picker, REVISIONS.items[0].sha);
    expect(downloadLink()).toHaveAttribute(
      "href",
      `/api/v1/projects/banskabystrica/export?format=zip&revision=${REVISIONS.items[0].sha}`,
    );
  });

  it("says secrets are never in a download", async () => {
    renderEndpoints();
    await userEvent.click(await screen.findByRole("button", { name: en.export.project }));
    expect(await screen.findByText(en.export.secretsNote)).toBeInTheDocument();
  });

  it("still downloads the current revision when the history is unavailable", async () => {
    renderEndpoints(503);

    await userEvent.click(await screen.findByRole("button", { name: en.export.project }));
    expect(await screen.findByText(en.export.noForge)).toBeInTheDocument();
    expect(downloadLink()).toHaveAttribute(
      "href",
      "/api/v1/projects/banskabystrica/export?format=zip",
    );
  });
});
