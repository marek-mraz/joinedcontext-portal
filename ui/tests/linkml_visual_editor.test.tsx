/** T-0218: authoring classes, slots and enums without leaving the document (DM-04…DM-06, DM-13, DM-16). */
import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { LinkmlVisualEditor } from "../src/pages/models/LinkmlVisualEditor";
import { diagnose, parseModel } from "../src/pages/models/linkml";

const SOURCE = `# the model the city publishes
id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots:
      - pm10
slots:
  pm10:
    range: float
    slot_uri: bb:pm10
enums: {}
`;

/** The editor is controlled, so the test holds the one document both views would share. */
function Harness({ initial = SOURCE }: { initial?: string }) {
  const [source, setSource] = useState(initial);
  return (
    <>
      <LinkmlVisualEditor
        source={source}
        onChange={setSource}
        diagnostics={diagnose(source, ["sk", "en"])}
        locales={["sk", "en"]}
      />
      <textarea readOnly aria-label="source" value={source} />
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
    source: () => (screen.getByLabelText("source") as HTMLTextAreaElement).value,
    user: userEvent.setup(),
  };
}

describe("LinkML visual editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("creates a class, and the YAML keeps the comment that was already there", async () => {
    const { source, user } = renderEditor();

    await user.type(screen.getByLabelText("New class"), "WeatherObserved");
    await user.click(screen.getByRole("button", { name: "Add class" }));

    expect(parseModel(source()).classes.map((klass) => klass.name)).toEqual([
      "AirQualityObserved",
      "WeatherObserved",
    ]);
    expect(source()).toContain("# the model the city publishes");
    expect(await screen.findByRole("button", { name: "WeatherObserved" })).toBeInTheDocument();
  });

  it("adds a slot to the open class and lists it with its range", async () => {
    const { source, user } = renderEditor();

    await user.type(screen.getByLabelText("New slot"), "pm25");
    const slots = screen.getByRole("table");
    await user.click(screen.getByRole("button", { name: "Add slot" }));

    const model = parseModel(source());
    expect(model.classes[0].slots).toEqual(["pm10", "pm25"]);
    expect(model.slots.find((slot) => slot.name === "pm25")?.range).toBe("string");
    expect(within(slots).getByRole("row", { name: /pm10/ })).toBeInTheDocument();
  });

  it("edits a slot's range, kind, unit and required flag through the document", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.selectOptions(screen.getByLabelText("Range"), "integer");
    await user.selectOptions(screen.getByLabelText("NGSI-LD kind"), "GeoProperty");
    await user.selectOptions(screen.getByLabelText("Unit"), "GQ");
    await user.click(screen.getByLabelText("Required"));

    const slot = parseModel(source()).slots.find((candidate) => candidate.name === "pm10");
    expect(slot?.range).toBe("integer");
    expect(slot?.kind).toBe("GeoProperty");
    expect(slot?.required).toBe(true);
    expect(slot?.unit?.ucum_code).toBe("ug/m3");
    // DM-06: the CEFACT common code travels with the unit, not only the UCUM symbol.
    expect(slot?.unit?.exact_mappings).toEqual(["ucefact:GQ"]);
    // DM-05: the kind is the annotation Model Tools reads, not a field of our own.
    expect(source()).toContain("ngsi_ld_kind: GeoProperty");
  });

  it("refuses to mint a slot IRI under a namespace that belongs to someone else", async () => {
    const { source, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.clear(screen.getByLabelText("Slot IRI"));
    await user.type(screen.getByLabelText("Slot IRI"), "sdm:pm10");

    expect(await screen.findByRole("alert")).toHaveTextContent("smartdatamodels.org");
    // The refused term never reaches the document, and the field keeps what was typed so the
    // person can correct it rather than watch keystrokes disappear.
    expect(source()).not.toContain("sdm:pm10");
    expect(screen.getByLabelText("Slot IRI")).toHaveValue("sdm:pm10");
  });

  it("lets an upstream slot keep the upstream IRI it cites", async () => {
    const cited = SOURCE.replace(
      "    slot_uri: bb:pm10",
      "    slot_uri: sdm:pm10\n    annotations:\n      upstream_source: https://github.com/smart-data-models/dataModel.Environment@9f1c2b7",
    );
    const { source, user } = renderEditor(cited);

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.clear(screen.getByLabelText("Slot IRI"));
    await user.type(screen.getByLabelText("Slot IRI"), "sdm:pm10");

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(parseModel(source()).slots[0].slot_uri).toBe("sdm:pm10");
  });

  it("writes a title per configured locale, and warns while one is missing", async () => {
    const { source, user } = renderEditor();

    await user.type(screen.getByLabelText("Title (sk)"), "Kvalita ovzdušia");

    expect(parseModel(source()).classes[0].title).toEqual({ sk: "Kvalita ovzdušia" });
    expect(
      await screen.findByText(/has no title in en/, { selector: "p" }),
    ).toBeInTheDocument();
  });

  it("adds an enum and a value to it", async () => {
    const { source, user } = renderEditor();

    await user.type(screen.getByLabelText("New enum"), "QualityBand");
    await user.click(screen.getByRole("button", { name: "Add enum" }));

    await user.type(await screen.findByLabelText("New value of QualityBand"), "good");
    await user.click(screen.getByRole("button", { name: "Add value to QualityBand" }));

    const model = parseModel(source());
    expect(model.enums[0].name).toBe("QualityBand");
    expect(model.enums[0].permissible_values.map((value) => value.name)).toEqual(["good"]);
  });
});
