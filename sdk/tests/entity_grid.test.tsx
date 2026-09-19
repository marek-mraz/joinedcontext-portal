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
      filter: "Filter",
      ops: {
        contains: "obsahuje",
        equals: "je",
        notEquals: "nie je",
        gt: ">",
        gte: "≥",
        lt: "<",
        lte: "≤",
        between: "medzi",
        empty: "je prázdne",
        present: "má hodnotu",
        pattern: "vyhovuje",
      },
      value: "Hodnota",
      upperValue: "Horná hodnota",
      query: "Čo sa pýta",
      copyQuery: "Kopírovať dotaz",
      editAsText: "Upraviť ako text",
      filterRow: "Filtre",
      sortPage: "Zoradiť túto stranu podľa",
      matching: "vyhovujúcich",
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
    const pairs: [string, string][] = Object.entries(DEFAULT_LABELS).flatMap(([key, english]) =>
      typeof english === "string"
        ? [[key, english] as [string, string]]
        : Object.entries(english).map(([op, name]) => [`${key}.${op}`, name] as [string, string]),
    );
    for (const [key, english] of pairs) {
      const host = key.startsWith("ops.")
        ? slovakLabels.ops[key.slice(4) as keyof typeof slovakLabels.ops]
        : slovakLabels[key as keyof typeof slovakLabels];
      if (english !== host) {
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

  /**
   * The filter row asks the endpoint, never the loaded page (T-1429, UI-66): each case reads the
   * query the source was given, because that is the only thing that decides what a person sees.
   */
  describe("the filter row", () => {
    /** A source that records every query the grid sends it. */
    function recordingSource(entities: Record<string, unknown>[]) {
      const asked: { q?: string; idPattern?: string; offset: number }[] = [];
      const inner = fixtureSource(entities);
      return {
        asked,
        source: {
          query: async (q: Parameters<typeof inner.query>[0], page: Parameters<typeof inner.query>[1]) => {
            asked.push({ q: q.q, idPattern: q.idPattern, offset: page.offset });
            return inner.query(q, page);
          },
          get: inner.get,
        },
      };
    }

    it("sends the query the chosen operator and value compose", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });

      await waitFor(() => {
        expect(asked.at(-1)?.q).toBe("availableBikeNumber>4");
      });
      // And it says so under the grid, where a person can copy it.
      expect(screen.getByText("q=availableBikeNumber>4")).toBeInTheDocument();
    });

    it("asks nothing until the filter is complete, and starts again at the first page", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={onePerPage} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.click(screen.getByText(DEFAULT_LABELS.next));
      await waitFor(() => expect(asked.at(-1)?.offset).toBe(1));

      // The operator alone narrows nothing: the value is still missing.
      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      await waitFor(() => expect(asked.at(-1)?.q).toBeUndefined());

      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      await waitFor(() => {
        expect(asked.at(-1)?.q).toBe("availableBikeNumber>4");
        // Page one, because page two of the narrowed answer may not exist.
        expect(asked.at(-1)?.offset).toBe(0);
      });
    });

    it("asks the id column by pattern rather than by a term of the query", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.change(screen.getByLabelText("Filter: ID"), { target: { value: "pattern" } });
      fireEvent.change(screen.getByLabelText("Value: ID"), { target: { value: "helsinki:002" } });

      await waitFor(() => {
        expect(asked.at(-1)?.idPattern).toBe("helsinki:002");
        expect(asked.at(-1)?.q).toBeUndefined();
      });
      // The endpoint answered about the set, and the grid shows what came back.
      await waitFor(() => expect(screen.queryByText("Kamppi")).toBeNull());
      expect(screen.getByText("Kallio")).toBeInTheDocument();
    });

    it("hands the query over as text, and sends what the person typed", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      await waitFor(() => expect(asked.at(-1)?.q).toBe("availableBikeNumber>4"));

      // The switch starts from what the row built, so nothing is lost by taking it over.
      fireEvent.click(screen.getByLabelText(DEFAULT_LABELS.editAsText));
      const field = screen.getByLabelText(DEFAULT_LABELS.query);
      expect(field).toHaveValue("availableBikeNumber>4");

      // And a `q` the row cannot show is exactly what the text field is for.
      fireEvent.change(field, { target: { value: 'availableBikeNumber>4|name=="Kallio"' } });
      await waitFor(() => expect(asked.at(-1)?.q).toBe('availableBikeNumber>4|name=="Kallio"'));
      // With the query in the person's hands the row's controls are gone, so the two cannot
      // disagree about what is being asked.
      expect(screen.queryByLabelText("Filter: Bikes")).toBeNull();
    });

    it("keeps a preset and a filter both true, and offers only the allowed columns", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(
        <EntityGrid
          config={{
            ...config,
            filters: { allowed: ["availableBikeNumber"], preset: { q: 'name=="Kamppi"' } },
          }}
          source={source}
        />,
      );
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      // Only the allowed column has a filter: a grid narrowed by its dashboard stays narrowed.
      expect(screen.queryByLabelText("Filter: Name")).toBeNull();

      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      await waitFor(() => expect(asked.at(-1)?.q).toBe('name=="Kamppi";availableBikeNumber>4'));
    });

    it("shows the endpoint's own count in the footer, and nothing when it sent none", async () => {
      const counted = {
        query: async () => ({ rows: [], total: 41 }),
        get: async () => null,
      };
      const { unmount } = render(<EntityGrid config={config} source={counted} />);
      expect(await screen.findByText(`41 ${DEFAULT_LABELS.matching}`)).toBeInTheDocument();
      unmount();

      // A narrowed answer carries no count (R22): the footer then pages without claiming a total.
      const uncounted = { query: async () => ({ rows: [] }), get: async () => null };
      render(<EntityGrid config={config} source={uncounted} />);
      await waitFor(() => expect(screen.getByText(DEFAULT_LABELS.empty)).toBeInTheDocument());
      expect(screen.queryByText(new RegExp(DEFAULT_LABELS.matching))).toBeNull();
    });

    it("offers no filter for a geometry, and none for a column of entity timestamps", async () => {
      render(<EntityGrid config={{ ...config, entityTimestamps: true }} source={fixtureSource(bikeEntities)} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      expect(screen.queryByLabelText("Filter: Location")).toBeNull();
      expect(screen.queryByLabelText(`Filter: ${DEFAULT_LABELS.createdAt}`)).toBeNull();
      expect(screen.getByLabelText("Filter: Bikes")).toBeInTheDocument();
    });

    it("says that sorting orders the loaded page", async () => {
      render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      const sort = screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} Bikes` });
      fireEvent.click(sort);
      // Ascending by the page's own values: 3 before 5, and the header says which way it went.
      const cells = screen.getAllByRole("gridcell").map((cell) => cell.textContent);
      expect(cells.indexOf("3 C62")).toBeLessThan(cells.indexOf("5 C62"));
      expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} Bikes` }).textContent).toContain("↑");
    });
  });
});
