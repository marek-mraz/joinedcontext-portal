/**
 * The catalog results (T-0579, UI-46, AG-58, AG-75): one compact row per item the assistant
 * found, four before "Show more", a "Use" that puts an endpoint into the conversation's data bar,
 * and every string drawn as text (AG-46).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

function renderCards(
  items: CatalogItem[] = ITEMS,
  props: { onUseEndpoint?: (name: string) => void; usedEndpoints?: string[]; words?: string } = {},
) {
  const rootRoute = createRootRoute({
    component: () => <CatalogCards project="helsinki" items={items} now={NOW} {...props} />,
  });
  const router = createRouter({ routeTree: rootRoute });
  return render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
}

function endpointItem(n: number): CatalogItem {
  return {
    kind: "Endpoint",
    name: `helsinki-${String(n)}`,
    space: "helsinki",
    owner: "helsinki",
    title: `Helsinki ${String(n)}`,
    matchReason: ["title"],
    access: { verdict: "allowed", reason: "audience public" },
    freshness: null,
  };
}

describe("the catalog results", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("draws one compact row per item: kind, title, name, an access dot and freshness when known", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(3);

    const bikes = rows[0];
    expect(within(bikes).getByRole("img", { name: "Endpoint" })).toBeInTheDocument();
    expect(within(bikes).getByText("helsinki-bikes")).toBeInTheDocument();
    expect(within(bikes).getByRole("img", { name: `${en.agentRun.catalog.allowed} · audience public` })).toBeInTheDocument();
    const fed = within(bikes).getByText("fed 12 s ago");
    expect(fed.getAttribute("title")).toBe("Fed 12 s ago by hsl-bikes");

    const partners = rows[1];
    expect(
      within(partners).getByRole("img", {
        name: `${en.agentRun.catalog.restricted} · audience project-list does not name helsinki`,
      }),
    ).toBeInTheDocument();
    expect(within(partners).queryByText(/^fed /)).toBeNull();

    expect(within(rows[2]).getByRole("img", { name: "ContextSpace" })).toBeInTheDocument();
  });

  it("keeps every row inside a narrow chat column: rows wrap and shrink, long text truncates with a title", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    for (const row of within(list).getAllByRole("listitem")) {
      expect(row).toHaveClass("min-w-0", "flex-wrap");
    }
    const bikes = within(list).getAllByRole("listitem")[0];
    const title = within(bikes).getByRole("link");
    expect(title).toHaveClass("min-w-0", "truncate");
    expect(title).toHaveAttribute("title", ITEMS[0].title);
    // The access reason rides on the dot's tooltip, never as a line that runs past the edge.
    expect(within(bikes).queryByText(/audience public/)).toBeNull();
  });

  it("links an open item's title to the explorer and a restricted one to its kind's page", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const rows = within(list).getAllByRole("listitem");

    const href = within(rows[0]).getByRole("link").getAttribute("href") ?? "";
    expect(href).toContain("/projects/helsinki/explore");
    expect(href).toContain("space=helsinki");
    expect(href).toContain("endpoint=helsinki-bikes");

    expect(within(rows[1]).getByRole("link", { name: "helsinki-partners" }).getAttribute("href")).toBe(
      "/projects/helsinki/endpoints",
    );

    const space = within(rows[2]).getByRole("link", { name: "Helsinki city context" }).getAttribute("href") ?? "";
    expect(space).toContain("space=helsinki");
    expect(space).not.toContain("endpoint=");
  });

  it("shows four rows, then the rest behind Show more", async () => {
    const user = userEvent.setup();
    renderCards([1, 2, 3, 4, 5, 6].map(endpointItem));
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    expect(within(list).getAllByRole("listitem")).toHaveLength(4);

    await user.click(screen.getByRole("button", { name: "Show 2 more" }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(6);

    await user.click(screen.getByRole("button", { name: en.agentRun.catalog.showLess }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(4);
  });

  it("offers Use on an open endpoint, spent once the conversation reads it; spaces and restricted rows have none", async () => {
    const user = userEvent.setup();
    const used: string[] = [];
    renderCards(ITEMS, {
      onUseEndpoint: (name) => {
        used.push(name);
      },
      usedEndpoints: ["helsinki-kpi"],
    });
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const rows = within(list).getAllByRole("listitem");

    await user.click(within(rows[0]).getByRole("button", { name: "Use helsinki-bikes in this conversation" }));
    expect(used).toEqual(["helsinki-bikes"]);
    expect(within(rows[1]).queryByRole("button")).toBeNull();
    expect(within(rows[2]).queryByRole("button")).toBeNull();
  });

  it("marks an endpoint already in the conversation as in use", async () => {
    renderCards(ITEMS, { onUseEndpoint: () => undefined, usedEndpoints: ["helsinki-bikes"] });
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    const button = within(within(list).getAllByRole("listitem")[0]).getByRole("button", {
      name: "helsinki-bikes is in use",
    });
    expect(button).toBeDisabled();
    expect(button).toHaveTextContent(en.agentRun.catalog.inUse);
  });

  it("offers no Use without a conversation to add to", async () => {
    renderCards();
    const list = await screen.findByRole("list", { name: en.agentRun.catalog.title });
    expect(within(list).queryAllByRole("button")).toHaveLength(0);
  });

  it("renders what the run stream carried as text, never as markup (AG-46)", async () => {
    renderCards();
    expect(await screen.findByText("Helsinki city bikes <script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });

  it("says when nothing matched, naming the words it searched for", async () => {
    renderCards([], { words: " electric scooters " });
    expect(
      await screen.findByText(
        "Nothing in this project matches “electric scooters”. Try a single word, such as a type or a place.",
      ),
    ).toBeInTheDocument();
  });

  it("says when nothing matched a search whose words it does not know", async () => {
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
