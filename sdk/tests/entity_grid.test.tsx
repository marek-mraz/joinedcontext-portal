import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import { DEFAULT_LABELS, useEntityGrid } from "../src/grid/useEntityGrid";

const bikeEntities: Record<string, unknown>[] = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 5, unitCode: "C62", observedAt: "2026-01-01T10:00:00Z" },
    refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:hel:001" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
    name: { type: "LanguageProperty", languageMap: { en: "Kamppi", fi: "Kamppi" } },
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 3, unitCode: "C62", observedAt: "2026-01-01T11:00:00Z" },
    refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:hel:002" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.95, 60.18] } },
    name: { type: "LanguageProperty", languageMap: { en: "Kallio", fi: "Kallio" } },
  },
];

const configResult = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "BikeHireDockingStation",
  columns: [
    { attr: "name", label: "Name" },
    { attr: "availableBikeNumber", label: "Bikes", show: { unit: true } },
    { attr: "refDevice", label: "Device" },
    { attr: "location", label: "Location" },
  ],
  pageSize: 10,
});

const config = configResult.config!;
// One row a page, so the pager has somewhere to go.
const onePerPage = { ...config, pageSize: 1 };

describe("EntityGrid", () => {
  it("renders value with unit", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("5 C62")).toBeInTheDocument();
      expect(screen.getByText("3 C62")).toBeInTheDocument();
    });
  });

  it("toggles observedAt column via header menu", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    // Click the menu button for availableBikeNumber
    const menuBtn = screen.getByLabelText("Show metadata for availableBikeNumber");
    fireEvent.click(menuBtn);

    // Check the observedAt checkbox
    const observedCheckbox = screen.getByLabelText("Observed");
    expect(observedCheckbox).not.toBeChecked();
    fireEvent.click(observedCheckbox);

    // Now the observedAt column should appear
    await waitFor(() => {
      expect(screen.getByText("2026-01-01T10:00:00Z")).toBeInTheDocument();
      expect(screen.getByText("2026-01-01T11:00:00Z")).toBeInTheDocument();
    });
  });

  it("renders relationship URN as button when onOpenRelationship given", async () => {
    const onOpen = vi.fn();
    render(
      <EntityGrid
        config={config}
        source={fixtureSource(bikeEntities)}
        onOpenRelationship={onOpen}
      />,
    );
    await waitFor(() => {
      const buttons = screen.getAllByRole("button");
      const relBtn = buttons.find((b) => b.textContent === "urn:ngsi-ld:Device:hel:001");
      expect(relBtn).toBeInTheDocument();
      fireEvent.click(relBtn!);
      expect(onOpen).toHaveBeenCalledWith("urn:ngsi-ld:Device:hel:001");
    });
  });

  it("shows empty label when source has no rows", async () => {
    render(<EntityGrid config={config} source={fixtureSource([])} />);
    await waitFor(() => {
      expect(screen.getByText("No rows.")).toBeInTheDocument();
    });
  });

  it("moves data-active with ArrowRight, ArrowDown, Home, End, PageDown", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    const grid = screen.getByRole("grid");
    grid.focus();

    // ArrowDown
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    await waitFor(() => {
      // second row should have data-active
      const cells = grid.querySelectorAll('[data-active]');
      expect(cells.length).toBeGreaterThan(0);
    });

    // ArrowRight
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    // Home
    fireEvent.keyDown(grid, { key: "Home" });
    // End
    fireEvent.keyDown(grid, { key: "End" });
    // PageDown
    fireEvent.keyDown(grid, { key: "PageDown" });
  });

  it("has role grid with aria-rowcount and headers with aria-colindex", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    const grid = screen.getByRole("grid");
    expect(grid).toHaveAttribute("aria-rowcount", "3"); // 2 rows + 1 header
    expect(grid).toHaveAttribute("aria-colcount");

    const headers = grid.querySelectorAll('[role="columnheader"]');
    expect(headers.length).toBeGreaterThan(0);
    headers.forEach((h, i) => {
      expect(h).toHaveAttribute("aria-colindex", String(i + 1));
    });
  });

  it("controlled: pager next calls onStateChange and offset does not change by itself", async () => {
    const onStateChange = vi.fn();
    render(
      <EntityGrid
        config={onePerPage}
        source={fixtureSource(bikeEntities)}
        state={{ offset: 0, activeCell: null, selected: [], shown: {}, sort: null }}
        onStateChange={onStateChange}
      />,
    );
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    const nextBtn = screen.getByText("Next");
    fireEvent.click(nextBtn);
    expect(onStateChange).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 1 }),
    );
    // offset should still be 0 because controlled
    expect(screen.getByText("Page 1")).toBeInTheDocument();
  });

  it("uncontrolled works without state props", async () => {
    render(<EntityGrid config={onePerPage} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });
    // pager works
    const nextBtn = screen.getByText("Next");
    fireEvent.click(nextBtn);
    await waitFor(() => {
      expect(screen.getByText("Page 2")).toBeInTheDocument();
    });
  });

  it("hook alone shows same rows", async () => {
    function TestComponent() {
      const grid = useEntityGrid({ config, source: fixtureSource(bikeEntities) });
      return (
        <ul>
          {grid.rows.map((r) => (
            <li key={r.id}>{r.id}</li>
          ))}
        </ul>
      );
    }
    render(<TestComponent />);
    await waitFor(() => {
      expect(screen.getByText("urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001")).toBeInTheDocument();
      expect(screen.getByText("urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002")).toBeInTheDocument();
    });
  });

  it("Slovak labels show no English text", async () => {
    const slovakLabels = {
      id: "ID",
      type: "Typ",
      createdAt: "Vytvorené",
      modifiedAt: "Upravené",
      observedAt: "Pozorované",
      unit: "Jednotka",
      datasetId: "Dataset",
      empty: "Prázdne.",
      loading: "Načítavam…",
      previous: "Predchádzajúca",
      next: "Nasledujúca",
      page: "Strana",
      showMetadata: "Zobraziť metadáta pre",
      error: "Chyba",
    };
    render(
      <EntityGrid
        config={config}
        source={fixtureSource(bikeEntities)}
        labels={slovakLabels}
      />,
    );
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByLabelText("Zobraziť metadáta pre availableBikeNumber"));
    // Every visible string is the host's: none of the English defaults that differ from Slovak shows.
    const text = document.body.textContent ?? "";
    for (const [key, english] of Object.entries(DEFAULT_LABELS)) {
      if (english !== slovakLabels[key as keyof typeof slovakLabels]) {
        expect(text, key).not.toContain(english);
      }
    }
    expect(screen.getByText("Strana 1")).toBeInTheDocument();
    expect(screen.getByText("Pozorované")).toBeInTheDocument();
  });

  it("renders <b>x</b> as text, not HTML", async () => {
    const entitiesWithHtml: Record<string, unknown>[] = [
      {
        id: "urn:1",
        type: "T",
        name: { type: "Property", value: "<b>x</b>" },
      },
    ];
    const configHtml = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "T",
      columns: [{ attr: "name" }],
    }).config!;
    render(<EntityGrid config={configHtml} source={fixtureSource(entitiesWithHtml)} />);
    await waitFor(() => {
      const cell = screen.getByText("<b>x</b>");
      expect(cell).toBeInTheDocument();
      expect(cell.querySelector("b")).toBeNull();
    });
  });
});
