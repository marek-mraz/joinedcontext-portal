/**
 * Disabled with a reason (T-0603, UI-44, PF-50): a denied control stays, disabled, and names
 * the verb and the kind by pointer and by keyboard; an allowed one is untouched; no document
 * yet, or a bootstrap administrator, leaves it enabled.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { PermissionGuard } from "../src/components/ui/PermissionGuard";

const VIEWER = { project: "helsinki", bootstrap: false, grants: [] };
const EDITOR = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "endpoint-editor", binding: "editors", rule: { kinds: ["Endpoint"], verbs: ["propose"] } }],
};

function renderGuard(permissions: unknown, kind = "Endpoint") {
  const fetchMock = vi.fn(() =>
    Promise.resolve(
      new Response(JSON.stringify(permissions), { status: 200, headers: { "Content-Type": "application/json" } }),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  const onClick = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <PermissionGuard project="helsinki" kind={kind} verb="propose">
          <button type="button" onClick={onClick}>
            New endpoint
          </button>
        </PermissionGuard>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, onClick };
}

describe("the permission guard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("disables a denied control, keeps it visible and names the missing verb and kind", async () => {
    const { onClick } = renderGuard(VIEWER);
    // The control is remounted inside the wrapper once the document arrives, so it is queried
    // after the wait, never held from before.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "New endpoint" })).toBeDisabled();
    });
    const button = screen.getByRole("button", { name: "New endpoint" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    const reason = "Disabled: your role does not permit 'propose' on 'Endpoint' in this project";
    expect(screen.getByRole("tooltip")).toHaveTextContent(reason);
    expect(button).toHaveAttribute("aria-describedby", screen.getByRole("tooltip").id);
    // By pointer: the wrapper's native tooltip; by keyboard: the wrapper takes focus.
    const wrapper = button.parentElement as HTMLElement;
    expect(wrapper).toHaveAttribute("title", reason);
    expect(wrapper).toHaveAttribute("tabindex", "0");
    await userEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("leaves an allowed control untouched", async () => {
    const { fetchMock, onClick } = renderGuard(EDITOR);
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalled();
    });
    const button = screen.getByRole("button", { name: "New endpoint" });
    expect(button).toBeEnabled();
    expect(screen.queryByRole("tooltip")).toBeNull();
    await userEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("guards by kind: the endpoint editor may not propose a pipeline", async () => {
    renderGuard(EDITOR, "Pipeline");
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "New endpoint" })).toBeDisabled();
    });
    expect(screen.getByRole("tooltip")).toHaveTextContent("'propose' on 'Pipeline'");
  });

  it("stays enabled for a bootstrap administrator", async () => {
    const { fetchMock } = renderGuard({ ...VIEWER, bootstrap: true });
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalled();
    });
    expect(screen.getByRole("button", { name: "New endpoint" })).toBeEnabled();
  });
});
