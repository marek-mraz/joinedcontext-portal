import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import {
  DEMO_LINKS,
  DEMO_SPACES,
  FederationPlayground,
  layoutSpaces,
} from "../src/components/visualization/FederationPlayground";
import type { FederationPlaygroundProps } from "../src/components/visualization/FederationPlayground";

function reducedMotion(reduce: boolean) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: reduce && query.includes("prefers-reduced-motion"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

describe("federation playground", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    reducedMotion(false);
    fetchMock = vi.fn(() => Promise.reject(new Error("the playground must not fetch")));
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function mount(props: FederationPlaygroundProps = {}) {
    return render(
      <I18nextProvider i18n={i18n}>
        <FederationPlayground {...props} />
      </I18nextProvider>,
    );
  }

  it("puts the hub in the middle and the other spaces on a ring around it", () => {
    const positions = layoutSpaces(DEMO_SPACES, 100);

    expect(positions["helsinki-hub"]).toEqual({ x: 50, y: 50 });
    const ring = DEMO_SPACES.filter((space) => !space.hub);
    const distances = ring.map((space) =>
      Math.hypot(positions[space.name].x - 50, positions[space.name].y - 50),
    );
    for (const distance of distances) {
      expect(distance).toBeCloseTo(34, 5);
    }
    // Every space has a place of its own: no two nodes are drawn on top of each other.
    const seen = new Set(ring.map((space) => `${positions[space.name].x}:${positions[space.name].y}`));
    expect(seen.size).toBe(ring.length);
  });

  it("draws one node per context space and one edge per registration", () => {
    mount();

    const graph = screen.getByRole("img", {
      name: `Federation graph: ${DEMO_SPACES.length} context spaces, ${DEMO_LINKS.length} registrations`,
    });
    expect(graph).toBeInTheDocument();
    for (const space of DEMO_SPACES) {
      expect(screen.getByRole("button", { name: space.name })).toBeInTheDocument();
    }
    for (const link of DEMO_LINKS) {
      expect(graph.querySelector(`[data-testid="edge-${link.from}-${link.to}"]`)).not.toBeNull();
    }
  });

  it("sends one answer along the first registration when the button is pressed", async () => {
    const user = userEvent.setup();
    mount();

    await user.click(screen.getByRole("button", { name: "Send a query" }));

    expect(
      screen.getByText(`${DEMO_LINKS[0].to} → ${DEMO_LINKS[0].from} · ${DEMO_LINKS[0].entityType}`),
    ).toBeInTheDocument();
    const graph = screen.getByRole("img", { name: /Federation graph/ });
    expect(graph.querySelector('[data-testid="mark-0"]')).not.toBeNull();
    expect(graph.querySelector('[data-testid="mark-motion"]')).not.toBeNull();
    expect(
      graph.querySelector(`[data-testid="edge-${DEMO_LINKS[0].from}-${DEMO_LINKS[0].to}"]`),
    ).toHaveAttribute("data-active", "true");
  });

  it("keeps sending while it runs and sends nothing while it is paused", async () => {
    // Fake timers and userEvent's own timer plumbing deadlock here, so the pause button is
    // pressed with a plain DOM click — the same control, one layer lower.
    vi.useFakeTimers();
    mount({ intervalMs: 1000 });

    await act(async () => {
      vi.advanceTimersByTime(2500);
    });
    const afterRunning = screen.getAllByRole("listitem").length;
    expect(afterRunning).toBeGreaterThanOrEqual(2);

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    });
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });

    expect(screen.getAllByRole("listitem")).toHaveLength(afterRunning);
    expect(screen.getByRole("button", { name: "Resume" })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows the registrations of the space a reader selects", async () => {
    const user = userEvent.setup();
    mount();

    await user.click(screen.getByRole("button", { name: "hsl-transport" }));

    const panel = screen.getByRole("heading", { name: "Registrations of hsl-transport" })
      .parentElement as HTMLElement;
    expect(within(panel).getByText("helsinki-hub → hsl-transport")).toBeInTheDocument();
    expect(within(panel).queryByText("helsinki-hub → energy")).toBeNull();
  });

  it("still reports the answer without animating it when the reader asked for less motion", async () => {
    reducedMotion(true);
    const user = userEvent.setup();
    mount();

    await user.click(screen.getByRole("button", { name: "Send a query" }));

    const graph = screen.getByRole("img", { name: /Federation graph/ });
    expect(graph.querySelector('[data-testid="mark-0"]')).not.toBeNull();
    expect(graph.querySelector('[data-testid="mark-motion"]')).toBeNull();
  });

  it("never touches the network", async () => {
    vi.useFakeTimers();
    mount({ intervalMs: 200 });

    await act(async () => {
      vi.advanceTimersByTime(2000);
    });

    expect(fetchMock).not.toHaveBeenCalled();
  });
});
