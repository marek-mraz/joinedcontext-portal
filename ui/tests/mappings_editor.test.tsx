/**
 * T-0223: writing a Mapping the reviewer can read (DM-33, DM-38, DM-40).
 *
 * The editor makes three promises and each one is a test here. It pre-fills what it can
 * justify and leaves the rest visibly empty, because a guessed derivation is worse than an
 * obvious gap. It shows one example transformed so the author sees the shape before CI does.
 * And it never lets a native Bloblang block look like ordinary mapped data: the block is
 * badged, the slot is named as unevaluated, and the review lane goes up.
 */
import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { MappingsEditor } from "../src/pages/models/MappingsEditor";
import type { MappingModel } from "../src/pages/models/MappingsEditor";
import { parseModel } from "../src/pages/models/linkml";
import {
  autoAlign,
  laneOf,
  nativeBlocks,
  toTransformationSpec,
  transform,
  unfilledRequired,
} from "../src/pages/models/mapping";
import type { Derivation } from "../src/pages/models/mapping";

/** What the city measures: micrograms, a Slovak band name and a station label. */
const CITY = `id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots: [pm10, band, stationName, temperature]
slots:
  pm10:
    range: float
    slot_uri: sdm:pm10
    unit:
      ucum_code: GQ
  band:
    range: string
    slot_uri: bb:band
  stationName:
    range: string
    slot_uri: bb:stationName
  temperature:
    range: float
    slot_uri: sdm:temperature
    unit:
      ucum_code: CEL
enums: {}
`;

/** What the partner publishes: milligrams, an English band, and a differently named label. */
const PARTNER = `id: https://kosice.sk/models/air
name: partner-air
prefixes:
  ks: https://kosice.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQuality:
    class_uri: ks:AirQuality
    slots: [pm10, quality, siteLabel, temperature]
slots:
  pm10:
    range: float
    slot_uri: sdm:pm10
    unit:
      ucum_code: M1
  quality:
    range: string
    slot_uri: ks:quality
    required: true
  siteLabel:
    range: string
    slot_uri: ks:siteLabel
  temperature:
    range: float
    slot_uri: sdm:temperature
    unit:
      ucum_code: CEL
enums: {}
`;

const MODELS: MappingModel[] = [
  { name: "air", version: "1.2.0", source: CITY },
  { name: "partner-air", version: "2.0.0", source: PARTNER },
];

function Harness({ models = MODELS }: { models?: MappingModel[] }) {
  const [spec, setSpec] = useState("");
  return (
    <I18nextProvider i18n={i18n}>
      <MappingsEditor
        models={models}
        alignments={[
          {
            subject_id: "bb:stationName",
            predicate_id: "skos:closeMatch",
            object_id: "ks:siteLabel",
          },
        ]}
        onChange={setSpec}
      />
      <textarea readOnly aria-label="spec" value={spec} />
    </I18nextProvider>
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the derivations the editor guesses", () => {
  it("aligns an identical slot name, the same term IRI, and an SSSOM close match", () => {
    const derivations = autoAlign(parseModel(CITY), parseModel(PARTNER), [
      { subject_id: "bb:stationName", predicate_id: "skos:closeMatch", object_id: "ks:siteLabel" },
    ]);
    const by = (name: string) => derivations.find((d) => d.target === name);

    // The same name on both sides is the strongest evidence there is.
    expect(by("pm10")).toMatchObject({ populatedFrom: "pm10", origin: "identical" });
    expect(by("temperature")).toMatchObject({ populatedFrom: "temperature", origin: "identical" });
    // Different names, same Smart Data Model term: still an exact match.
    expect(by("siteLabel")).toMatchObject({ populatedFrom: "stationName", origin: "close" });
    // Nothing to go on, so nothing is invented.
    expect(by("quality")).toMatchObject({ origin: "manual" });
    expect(by("quality")?.populatedFrom).toBeUndefined();
  });

  it("notices the two slots measure in different units and asks rather than guesses", () => {
    const derivations = autoAlign(parseModel(CITY), parseModel(PARTNER));
    const pm10 = derivations.find((d) => d.target === "pm10");
    expect(pm10?.unitConversion).toMatchObject({ fromUnit: "GQ", toUnit: "M1" });
    // A guessed factor is a silently wrong measurement; 1 is the identity the author replaces.
    expect(pm10?.unitConversion?.factor).toBe(1);

    // Same unit on both sides is no conversion at all, not a conversion by one.
    const temperature = derivations.find((d) => d.target === "temperature");
    expect(temperature?.unitConversion).toBeUndefined();
  });

  it("names the required target slot that nothing fills (DM-34)", () => {
    const target = parseModel(PARTNER);
    const derivations = autoAlign(parseModel(CITY), target);
    expect(unfilledRequired(target, derivations)).toEqual(["quality"]);
  });
});

describe("what the derivations do to one example", () => {
  const derivations: Derivation[] = [
    {
      target: "pm10",
      populatedFrom: "pm10",
      origin: "identical",
      unitConversion: { factor: 0.001, fromUnit: "GQ", toUnit: "M1" },
    },
    {
      target: "quality",
      populatedFrom: "band",
      origin: "manual",
      valueMappings: { dobra: "good", zla: "poor" },
    },
    { target: "siteLabel", populatedFrom: "stationName", origin: "close" },
  ];

  it("converts the unit, remaps the value and carries the identity through", () => {
    const { output } = transform(
      { id: "urn:ngsi-ld:AirQualityObserved:x", type: "AirQualityObserved", pm10: 34, band: "dobra", stationName: "Štiavničky" },
      derivations,
    );
    expect(output).toEqual({
      id: "urn:ngsi-ld:AirQualityObserved:x",
      type: "AirQualityObserved",
      pm10: 0.034,
      quality: "good",
      siteLabel: "Štiavničky",
    });
  });

  it("leaves a value the author never mapped alone rather than dropping it", () => {
    const { output } = transform({ band: "neznama" }, derivations);
    expect(output.quality).toBe("neznama");
  });

  it("reports a source slot the example does not carry instead of writing undefined", () => {
    const { output, missing } = transform({ pm10: 10 }, derivations);
    expect(missing).toEqual(["quality", "siteLabel"]);
    expect("quality" in output).toBe(false);
  });

  it("never evaluates a native block, and says which slot it did not evaluate (DM-38)", () => {
    const withNative: Derivation[] = [
      ...derivations,
      { target: "aqi", origin: "manual", native: 'root.aqi = this.pm10 * 2' },
    ];
    const { output, unchecked } = transform({ pm10: 34 }, withNative);
    expect(unchecked).toEqual(["aqi"]);
    expect("aqi" in output).toBe(false);
  });
});

describe("what a native block costs", () => {
  it("raises the lane by exactly one level, however many blocks there are (DM-38)", () => {
    const clean: Derivation[] = [{ target: "pm10", populatedFrom: "pm10", origin: "identical" }];
    const one: Derivation[] = [...clean, { target: "aqi", origin: "manual", native: "root = this" }];
    const two: Derivation[] = [...one, { target: "band", origin: "manual", native: "root = this" }];

    expect(laneOf(clean)).toBe("green");
    expect(laneOf(one)).toBe("yellow");
    expect(laneOf(two)).toBe("yellow");
    expect(laneOf(one, "yellow")).toBe("red");
    // Whitespace is not a block.
    expect(laneOf([{ target: "x", origin: "manual", native: "   " }])).toBe("green");
  });

  it("collects the blocks as spec.native[] entries, one per target slot", () => {
    expect(
      nativeBlocks([
        { target: "pm10", populatedFrom: "pm10", origin: "identical" },
        { target: "aqi", origin: "manual", native: "root.aqi = this.pm10" },
      ]),
    ).toEqual([{ targetSlot: "aqi", language: "bloblang", source: "root.aqi = this.pm10" }]);
  });
});

describe("the specification that gets saved", () => {
  it("writes LinkML-Map slot derivations and nothing from another language (DM-33)", () => {
    const spec = toTransformationSpec(
      [
        {
          target: "pm10",
          populatedFrom: "pm10",
          origin: "identical",
          unitConversion: { factor: 0.001, fromUnit: "GQ", toUnit: "M1" },
          cast: "float",
        },
        { target: "quality", populatedFrom: "band", origin: "manual", valueMappings: { dobra: "good" } },
        { target: "unmapped", origin: "manual" },
      ],
      "AirQualityObserved",
      "AirQuality",
    );

    expect(spec).toContain("class_derivations:");
    expect(spec).toContain("populated_from: AirQualityObserved");
    expect(spec).toContain("unit_conversion:");
    expect(spec).toContain("factor: 0.001");
    expect(spec).toContain("value_mappings:");
    expect(spec).toContain("dobra: good");
    expect(spec).toContain("range: float");
    // A slot nothing fills is not written as filled by nothing.
    expect(spec).not.toContain("unmapped:");
  });
});

describe("the editor on screen", () => {
  it("shows one row per target slot with the source it guessed", async () => {
    render(<Harness />);
    const canvas = screen.getByRole("table");
    const pm10 = within(canvas).getByLabelText("Source slot for pm10");
    expect((pm10 as HTMLSelectElement).value).toBe("pm10");
    const quality = within(canvas).getByLabelText("Source slot for quality");
    expect((quality as HTMLSelectElement).value).toBe("");
  });

  it("offers a unit dropdown only where the two slots disagree about units", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Target unit for pm10")).toBeInTheDocument();
    expect(screen.queryByLabelText("Target unit for temperature")).not.toBeInTheDocument();
  });

  it("refuses to call the mapping finished while a required slot is unfilled (DM-34)", () => {
    render(<Harness />);
    expect(screen.getByRole("alert")).toHaveTextContent("quality");
  });

  it("transforms the example live as the author connects a slot (DM-40)", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const input = screen.getByLabelText("Input example (JSON)");
    await user.clear(input);
    await user.type(
      input,
      '{{"type": "AirQualityObserved", "band": "dobra", "stationName": "Stiavnicky"}',
    );

    // `siteLabel` was pre-filled from the SSSOM close match, so it is already in the output.
    const output = screen.getByLabelText("Transformed output");
    expect(output).toHaveTextContent("Stiavnicky");

    // Connecting `quality` to `band` shows up in the same place, without a save.
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    expect(screen.getByLabelText("Transformed output")).toHaveTextContent("dobra");
  });

  it("says so plainly when the example is not JSON, rather than showing an empty result", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const input = screen.getByLabelText("Input example (JSON)");
    await user.clear(input);
    await user.type(input, "not json");
    expect(screen.getByText("The example is not valid JSON.")).toBeInTheDocument();
  });

  it("writes the specification out as the author edits it", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    const spec = screen.getByLabelText("spec") as HTMLTextAreaElement;
    expect(spec.value).toContain("populated_from: band");
    expect(spec.value).toContain("class_derivations:");
  });
});
