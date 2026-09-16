/**
 * T-0796: a manifest is its path, so renaming one in a form would propose a second resource and
 * leave the first behind. The dialog fixes the name of a resource that exists — in the form and
 * in the YAML view, which is the one that used to get through (MF-11, MF-12).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />;
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));
vi.mock("maplibre-gl", () => {
  class FakeMap {
    addControl() {}
    addSource() {}
    addLayer() {}
    on(event: string, second: unknown) {
      if (event === "load" && typeof second === "function") {
        (second as () => void)();
      }
    }
    remove() {}
    getSource() {
      return undefined;
    }
    removeLayer() {}
    removeSource() {}
    fitBounds() {}
    getBounds() {
      return { getWest: () => 24.9, getSouth: () => 60.1, getEast: () => 25, getNorth: () => 60.2 };
    }
  }
  const Popup = class {
    setLngLat() {
      return this;
    }
    setHTML() {
      return this;
    }
    addTo() {
      return this;
    }
  };
  return { Map: FakeMap, NavigationControl: class {}, Popup, default: { Map: FakeMap, NavigationControl: class {}, Popup } };
});

const { App } = await import("../src/App");

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: [] };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const DASHBOARD = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Dashboard",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: { title: { en: "Bikes" }, visibility: "project", pages: [{ layout: "full-map", layers: [] }] },
};

function renderDashboards() {
  const writes: Request[] = [];
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const url = typeof input === "string" ? new URL(input, "http://localhost") : new URL(request.url);
    if (request instanceof Request && request.method !== "GET") {
      writes.push(request);
    }
    const json = (body: unknown) =>
      Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/dashboards")) {
      return json(list([DASHBOARD]));
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
  return writes;
}

describe("renaming a resource that exists", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/helsinki/dashboards");
  });

  it("is closed in the form and refused from the YAML view", async () => {
    const writes = renderDashboards();
    await userEvent.click(await screen.findByRole("button", { name: en.dashboards.edit }));
    const dialog = await screen.findByRole("dialog");

    expect(within(dialog).getByLabelText(new RegExp(`^${en.dashboards.field.name}`))).toHaveAttribute("readonly");

    await userEvent.click(within(dialog).getByRole("tab", { name: en.form.view.yaml }));
    const yaml = await within(dialog).findByLabelText("YAML");
    await userEvent.clear(yaml);
    await userEvent.click(yaml);
    // `paste`, not `type`: userEvent reads `{` and `[` in the text as key descriptors.
    await userEvent.paste(
      [
        "apiVersion: joinedcontext.com/v1alpha1",
        "kind: Dashboard",
        "metadata:",
        "  name: air-quality",
        "  namespace: helsinki",
        "spec:",
        "  visibility: project",
        "  pages:",
        "    - layout: full-map",
        "",
      ].join("\n"),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.dashboards.propose }));

    expect(await within(dialog).findByText(/Keep the name bikes/)).toBeInTheDocument();
    expect(writes).toHaveLength(0);
  });
});
