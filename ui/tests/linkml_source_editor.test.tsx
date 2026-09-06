/** T-0219: the YAML view of the same document, with the metamodel's own diagnostics (DM-13, DM-14). */
import { useEffect, useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom. The stand-in
// is a textarea with the same contract, so what the test exercises is this component's own
// work: the value it holds, the markers it computes and the completions it offers.
const monaco = vi.hoisted(() => ({
  setModelMarkers: vi.fn(),
  registerCompletionItemProvider: vi.fn(),
}));

function MockEditor({
  value,
  onChange,
  onMount,
}: {
  value: string;
  onChange?: (value: string) => void;
  onMount?: (editor: unknown, api: unknown) => void;
}) {
    useEffect(() => {
      onMount?.(
        { getModel: () => ({ uri: "inmemory://model" }) },
        {
          editor: { setModelMarkers: monaco.setModelMarkers },
          languages: {
            registerCompletionItemProvider: monaco.registerCompletionItemProvider,
            CompletionItemKind: { Property: 9 },
          },
        },
      );
      // The stand-in mounts once, exactly as the editor it replaces does.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
  return (
    <textarea
      aria-label="LinkML source"
      value={value}
      onChange={(event) => onChange?.(event.target.value)}
    />
  );
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

const { LinkmlSourceEditor, completionsFor, markersFrom, MARKER_SEVERITY } = await import(
  "../src/pages/models/LinkmlSourceEditor"
);
const { LinkmlVisualEditor } = await import("../src/pages/models/LinkmlVisualEditor");
const { diagnose, parseModel } = await import("../src/pages/models/linkml");

const SOURCE = `id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots:
      - pm10
slots:
  pm10:
    range: float
    slot_uri: bb:pm10
`;

/** Both views over one string, which is what the page holds. */
function Harness({ initial = SOURCE }: { initial?: string }) {
  const [source, setSource] = useState(initial);
  const diagnostics = diagnose(source);
  return (
    <>
      <LinkmlSourceEditor source={source} onChange={setSource} diagnostics={diagnostics} />
      <LinkmlVisualEditor source={source} onChange={setSource} diagnostics={diagnostics} />
    </>
  );
}

function renderEditor(initial?: string) {
  render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return {
    area: () => screen.getByLabelText("LinkML source") as HTMLTextAreaElement,
    user: userEvent.setup(),
  };
}

describe("LinkML source editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    monaco.setModelMarkers.mockClear();
    monaco.registerCompletionItemProvider.mockClear();
  });

  it("turns diagnostics into markers on the line they are about", () => {
    const markers = markersFrom([
      { line: 12, column: 3, severity: "error", message: "no such slot" },
      { line: 4, column: 1, severity: "warning", message: "no class_uri" },
    ]);
    expect(markers[0]).toMatchObject({
      startLineNumber: 12,
      startColumn: 3,
      message: "no such slot",
      severity: MARKER_SEVERITY.error,
    });
    expect(markers[1].severity).toBe(MARKER_SEVERITY.warning);
  });

  it("completes the metamodel, and the values a key takes where the line names one", () => {
    expect(completionsFor("  ").map((entry) => entry.label)).toContain("slot_uri");
    expect(completionsFor("      ngsi_ld_kind: ").map((entry) => entry.label)).toEqual([
      "Property",
      "GeoProperty",
      "Relationship",
      "LanguageProperty",
      "ListProperty",
      "JsonProperty",
      "VocabProperty",
    ]);
    expect(completionsFor("    range: fl").map((entry) => entry.label)).toContain("float");
    expect(completionsFor("      ucum_code: ").map((entry) => entry.detail)).toContain(
      "GQ · microgram per cubic metre",
    );
  });

  it("paints the model's markers into the editor and registers the completions", async () => {
    renderEditor(SOURCE.replace("    class_uri: bb:AirQualityObserved\n", ""));

    // The editor is loaded lazily, so the markers arrive on the tick after the first render.
    await waitFor(() =>
      expect(monaco.registerCompletionItemProvider).toHaveBeenCalledWith(
        "yaml",
        expect.anything(),
      ),
    );
    const markers = monaco.setModelMarkers.mock.calls.at(-1)?.[2] as { message: string }[];
    expect(markers.some((marker) => marker.message.includes("class_uri"))).toBe(true);
  });

  it("shows a YAML syntax error with the line it is on", async () => {
    renderEditor("classes:\n  Air:\n   slots: [pm10\n");

    expect(await screen.findByText(/line 4:/)).toBeInTheDocument();
    expect(
      await screen.findByText(/must be sufficiently indented/),
    ).toBeInTheDocument();
  });

  it("carries an edit made in the source through to the structured view", async () => {
    const { area, user } = renderEditor();

    await user.clear(area());
    await user.paste(SOURCE.replace("      - pm10", "      - pm10\n      - pm25").replace(
      "    slot_uri: bb:pm10\n",
      "    slot_uri: bb:pm10\n  pm25:\n    range: float\n    slot_uri: bb:pm25\n",
    ));

    expect(await screen.findByRole("button", { name: "pm25" })).toBeInTheDocument();
  });

  it("carries an edit made in the structured view through to the source", async () => {
    const { area, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.selectOptions(screen.getByLabelText("NGSI-LD kind"), "GeoProperty");

    expect(area().value).toContain("ngsi_ld_kind: GeoProperty");
    expect(parseModel(area().value).slots[0].kind).toBe("GeoProperty");
  });

  it("says so when the model has no open problem", async () => {
    renderEditor(SOURCE);
    expect(await screen.findByText("The model has no open problems.")).toBeInTheDocument();
  });
});
