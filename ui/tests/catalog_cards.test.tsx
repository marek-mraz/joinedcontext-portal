/**
 * The catalog cards (T-0579, UI-46, AG-58): one card per item the assistant found, with the
 * badges, the freshness, the links, and every string drawn as text (AG-46).
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CatalogCards, ago, catalogItemsOf } from "../src/pages/apps/CatalogCards";
import type { CatalogItem } from "../src/pages/apps/CatalogCards";

const NOW = Date.parse("2026-09-13T08:00:12Z");

const ITEMS: CatalogItem[] = [
  {
    kind: "Endpoint",
    name: "helsinki-bikes",
    space: "helsinki",
    owner: "helsinki",
    title: "Helsinki city bikes <script>alert(1)</script>",
    endpointSlug: "bikesslug00000000000000000000000",
    matchReason: ["title", "description"],
    access: { verdict: "allowed", reason: "audience public" },
    freshness: { pipeline: "hsl-bikes", scrapedAt: "2026-09-13T08:00:00Z", received: 1200, errors: 0 },
  },
  {
    kind: "Endpoint",
    name: "helsinki-partners",
    space: "helsinki",
    owner: "helsinki",
    matchReason: ["title"],
    access: { verdict: "restricted", reason: "audience project-list does not name helsinki" },
    freshness: null,
  },
  {
    kind: "ContextSpace",
    name: "helsinki",
    space: "helsinki",
    owner: "helsinki",
    title: "Helsinki city context",
    matchReason: ["endpoint helsinki-bikes"],
    access: { verdict: "allowed", reason: "endpoint helsinki-bikes admits you" },
    freshness: null,
  },
];

function renderCards(items: CatalogItem[] = ITEMS) {
  const rootRoute = createRootRoute({
    component: () => <CatalogCards project="helsinki" items={items} now={NOW} />,
  });
  const router = createRouter({ routeTree: rootRoute });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}

describe("the catalog cards", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("draws one card per item with its kind, owner, access and freshness", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const cards = within(list).getAllByRole("listitem");
    expect(cards).toHaveLength(3);

    const bikes = cards[0];
    expect(within(bikes).getByText("Endpoint")).toBeInTheDocument();
    expect(within(bikes).getByText(/helsinki-bikes · owner helsinki/)).toBeInTheDocument();
    expect(within(bikes).getByText(en.agentRun.catalog.allowed)).toBeInTheDocument();
    expect(within(bikes).getByText(/audience public/)).toBeInTheDocument();
    expect(within(bikes).getByText("Fed 12 s ago by hsl-bikes")).toBeInTheDocument();

    const partners = cards[1];
    expect(within(partners).getByText(en.agentRun.catalog.restricted)).toBeInTheDocument();
    expect(within(partners).getByText(/does not name helsinki/)).toBeInTheDocument();
    expect(within(partners).getByText(en.agentRun.catalog.noFeed)).toBeInTheDocument();
  });

  it("links an open item to the explorer on its space and endpoint, and a restricted one only to its page", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const cards = within(list).getAllByRole("listitem");

    const explore = within(cards[0]).getByRole("link", { name: en.agentRun.catalog.explore });
    const href = explore.getAttribute("href") ?? "";
    expect(href).toContain("/projects/helsinki/explore");
    expect(href).toContain("space=helsinki");
    expect(href).toContain("endpoint=helsinki-bikes");
    expect(within(cards[0]).getByRole("link", { name: en.agentRun.catalog.open }).getAttribute("href")).toBe(
      "/projects/helsinki/endpoints",
    );

    expect(within(cards[1]).queryByRole("link", { name: en.agentRun.catalog.explore })).toBeNull();
    expect(within(cards[1]).getByRole("link", { name: en.agentRun.catalog.open })).toBeInTheDocument();

    const space = within(cards[2]).getByRole("link", { name: en.agentRun.catalog.explore });
    expect(space.getAttribute("href")).toContain("space=helsinki");
    expect(space.getAttribute("href")).not.toContain("endpoint=");
    expect(within(cards[2]).getByRole("link", { name: en.agentRun.catalog.open }).getAttribute("href")).toBe(
      "/projects/helsinki/spaces",
    );
  });

  it("renders what the run stream carried as text, never as markup (AG-46)", async () => {
    renderCards();
    expect(await screen.findByText("Helsinki city bikes <script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });

  it("says when nothing matched", async () => {
    renderCards([]);
    expect(await screen.findByText(en.agentRun.catalog.none)).toBeInTheDocument();
  });

  it("takes only the items the Portal wrote", () => {
    expect(catalogItemsOf({ items: ITEMS })).toHaveLength(3);
    expect(catalogItemsOf({ items: [{ kind: "Endpoint" }, "x", null] })).toHaveLength(0);
    expect(catalogItemsOf("items")).toHaveLength(0);
    expect(catalogItemsOf(null)).toHaveLength(0);
  });

  it("formats a relative time from an instant and leaves other text alone", () => {
    const t = (key: string, values?: Record<string, unknown>) => `${key}:${String(values?.n)}`;
    expect(ago("2026-09-13T08:00:00Z", NOW, t)).toBe("agentRun.catalog.agoSeconds:12");
    expect(ago("2026-09-13T07:55:00Z", NOW, t)).toBe("agentRun.catalog.agoMinutes:5");
    expect(ago("2026-09-13T05:00:00Z", NOW, t)).toBe("agentRun.catalog.agoHours:3");
    expect(ago("2026-09-11T08:00:00Z", NOW, t)).toBe("agentRun.catalog.agoDays:2");
    expect(ago("not a date", NOW, t)).toBe("not a date");
  });
});
