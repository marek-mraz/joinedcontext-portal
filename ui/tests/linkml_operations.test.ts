/** T-0575: the operations a person, a test and an assistant edit a model with (DM-13, DM-16, DM-31). */
import { describe, expect, it } from "vitest";
import { parseModel } from "../src/pages/models/linkml";
import { applyOperations } from "../src/pages/models/operations";
import type { Operation } from "../src/pages/models/operations";

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

function slotNamed(source: string, name: string) {
  return parseModel(source).slots.find((slot) => slot.name === name);
}

describe("LinkML operations", () => {
  it("adds a class and a slot with two operations, and keeps the comment", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "addClass", name: "WeatherObserved", class_uri: "bb:WeatherObserved" },
      { op: "addSlot", name: "temperature", class: "WeatherObserved", range: "float", kind: "Property" },
    ]);
    expect(refused).toEqual([]);
    const model = parseModel(source);
    expect(model.classes.map((klass) => klass.name)).toEqual(["AirQualityObserved", "WeatherObserved"]);
    expect(model.classes[1].slots).toEqual(["temperature"]);
    expect(model.classes[1].class_uri).toBe("bb:WeatherObserved");
    expect(slotNamed(source, "temperature")?.range).toBe("float");
    expect(source).toContain("# the model the city publishes");
  });

  it("sets every slot field the form offers", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "setSlot", name: "pm10", field: "required", value: true },
      { op: "setSlot", name: "pm10", field: "multivalued", value: true },
      { op: "setSlot", name: "pm10", field: "description", value: "particulate matter" },
      { op: "setSlot", name: "pm10", field: "unit", value: "GQ" },
      { op: "setSlot", name: "pm10", field: "kind", value: "Property" },
      { op: "setSlot", name: "pm10", field: "minimum_value", value: 0 },
      { op: "setTitle", target: "slot", name: "pm10", locale: "sk", value: "PM10" },
      { op: "setTitle", target: "class", name: "AirQualityObserved", locale: "en", value: "Air quality" },
    ]);
    expect(refused).toEqual([]);
    const slot = slotNamed(source, "pm10");
    expect(slot).toMatchObject({
      required: true,
      multivalued: true,
      description: "particulate matter",
      unit: { ucum_code: "ug/m3", exact_mappings: ["ucefact:GQ"] },
      kind: "Property",
      minimum_value: 0,
      title: { sk: "PM10" },
    });
    expect(parseModel(source).classes[0].title).toEqual({ en: "Air quality" });
    // The default kind is not written: a Property annotation would be noise in every model.
    expect(source).not.toContain("ngsi_ld_kind");

    const cleared = applyOperations(source, [
      { op: "setSlot", name: "pm10", field: "required", value: false },
      { op: "setSlot", name: "pm10", field: "unit", value: "" },
      { op: "setSlot", name: "pm10", field: "kind", value: "GeoProperty" },
    ]).source;
    expect(slotNamed(cleared, "pm10")).toMatchObject({ required: false, unit: undefined, kind: "GeoProperty" });
  });

  it("removes, attaches and detaches, and enums get their values", () => {
    const { source, refused } = applyOperations(SOURCE, [
      { op: "addEnum", name: "QualityBand" },
      { op: "addEnumValue", enum: "QualityBand", value: "good", description: "below the limit" },
      { op: "addSlot", name: "band", range: "QualityBand" },
      { op: "attachSlot", class: "AirQualityObserved", slot: "band" },
      { op: "attachSlot", class: "AirQualityObserved", slot: "band" },
      { op: "detachSlot", class: "AirQualityObserved", slot: "pm10" },
      { op: "removeSlot", name: "pm10" },
    ]);
    expect(refused).toEqual([]);
    const model = parseModel(source);
    expect(model.classes[0].slots).toEqual(["band"]);
    expect(model.slots.map((slot) => slot.name)).toEqual(["band"]);
    expect(model.enums[0].permissible_values).toEqual([
      { name: "good", meaning: undefined, description: "below the limit" },
    ]);

    const gone = applyOperations(source, [{ op: "removeClass", name: "AirQualityObserved" }]).source;
    expect(parseModel(gone).classes).toEqual([]);
  });

  it("renames a slot in every class and a class in every reference, keeping places and comments", () => {
    const withReference = SOURCE.replace(
      "enums: {}\n",
      "  station:\n    range: AirQualityObserved\nenums: {}\n",
    ).replace("    class_uri: bb:AirQualityObserved\n", "    class_uri: bb:AirQualityObserved\n    is_a: Entity\n");
    const { source, refused } = applyOperations(withReference, [
      { op: "addClass", name: "Reading", is_a: "AirQualityObserved" },
      { op: "renameSlot", name: "pm10", to: "particles" },
      { op: "renameClass", name: "AirQualityObserved", to: "AirQuality" },
    ]);
    expect(refused).toEqual([]);
    const model = parseModel(source);
    expect(model.classes.map((klass) => klass.name)).toEqual(["AirQuality", "Reading"]);
    expect(model.classes[0].slots).toEqual(["particles"]);
    expect(model.slots.map((slot) => slot.name)).toEqual(["particles", "station"]);
    expect(slotNamed(source, "particles")?.range).toBe("float");
    expect(slotNamed(source, "station")?.range).toBe("AirQuality");
    expect(source).toContain("is_a: Entity");
    expect(source).toContain("  Reading:\n    is_a: AirQuality");
    expect(source).toContain("# the model the city publishes");

    expect(
      applyOperations(SOURCE, [
        { op: "renameSlot", name: "pm25", to: "x" },
        { op: "renameClass", name: "AirQualityObserved", to: "AirQualityObserved" },
      ]).refused.map((refusal) => refusal.reason),
    ).toEqual(["unknown slot 'pm25'", "class 'AirQualityObserved' already exists"]);
  });

  it("adds a slot with its kind and an entity class in one operation each", () => {
    // T-0599: inference answers `addSlot` with a kind and `addClass` with `is_a`, so the two
    // must land in one operation, not need a second `setSlot`.
    const applied = applyOperations(SOURCE, [
      { op: "addClass", name: "Reading", is_a: "Entity" },
      { op: "addSlot", name: "parent", class: "Reading", range: "uriorcurie", kind: "Relationship" },
    ]);

    expect(applied.refused).toEqual([]);
    const model = parseModel(applied.source);
    expect(model.slots.find((slot) => slot.name === "parent")?.kind).toBe("Relationship");
    expect(applied.source).toContain("is_a: Entity");
  });

  it("refuses by name, and then nothing of the list lands", () => {
    const operations: Operation[] = [
      { op: "addClass", name: "WeatherObserved" },
      { op: "attachSlot", class: "Nope", slot: "pm10" },
      { op: "setSlot", name: "pm25", field: "range", value: "float" },
      { op: "setSlot", name: "pm10", field: "slot_uri", value: "sdm:pm10" },
      { op: "setSlot", name: "pm10", field: "range", value: "Nowhere" },
      { op: "setSlot", name: "pm10", field: "unit", value: "XX" },
      { op: "addClass", name: "not a name" },
      { op: "addSlot", name: "pm10" },
      { op: "addEnumValue", enum: "Missing", value: "x" },
    ];
    const { source, refused } = applyOperations(SOURCE, operations);
    expect(source).toBe(SOURCE);
    expect(refused.map((refusal) => refusal.index)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(refused.map((refusal) => refusal.reason)).toEqual([
      "unknown class 'Nope'",
      "unknown slot 'pm25'",
      "slot 'pm10' would mint 'sdm:pm10' under https://smartdatamodels.org/, which belongs to someone else",
      "range 'Nowhere' of slot 'pm10' is neither a type, an enum nor a class of this model",
      "unit 'XX' of slot 'pm10' is not a known UN/CEFACT common code",
      "'not a name' is not a valid class name: letters, digits and _ only, starting with a letter, " +
        "because the name becomes the entity type in every URN. Put the readable name in its title.",
      "slot 'pm10' already exists",
      "unknown enum 'Missing'",
    ]);
  });

  it("lets a slot cite an upstream term under a reserved namespace, and refuses a broken source", () => {
    const cited = SOURCE.replace(
      "  pm10:\n",
      "  no2:\n    annotations:\n      upstream_source: sdm\n  pm10:\n",
    );
    const { refused } = applyOperations(cited, [
      { op: "setSlot", name: "no2", field: "slot_uri", value: "sdm:no2" },
    ]);
    expect(refused).toEqual([]);

    const broken = applyOperations("classes: [\n", [{ op: "addClass", name: "X" }]);
    expect(broken.source).toBe("classes: [\n");
    expect(broken.refused).toEqual([{ index: 0, reason: "the source does not parse as YAML" }]);
  });
});

/**
 * T-1091: a class name becomes the entity type in every URN the space mints, and the gateway
 * accepts `^[A-Za-z][A-Za-z0-9_-]*$`, so an accented name would mint ids its own endpoint
 * refuses. The refusal says that, and says where the accented name belongs.
 */
describe("a name that could not survive a URN", () => {
  it("refuses an accented class name and says where the readable name goes", () => {
    const { refused } = applyOperations(SOURCE, [
      { op: "addClass", name: "Ovzduší" } as Operation,
    ]);
    expect(refused).toHaveLength(1);
    const reason = refused[0].reason;
    expect(reason).toContain("Ovzduší");
    expect(reason).toContain("entity type");
    expect(reason).toContain("title");
  });

  it("refuses the other shapes a name cannot take, and accepts the ones it can", () => {
    for (const name of ["东京", "Ärzte", "2Wheels", "air quality", "", "air-quality"]) {
      const { refused } = applyOperations(SOURCE, [{ op: "addClass", name } as Operation]);
      expect(refused, `${name} is not a usable class name`).toHaveLength(1);
    }
    for (const name of ["NoiseObserved", "_internal", "Bike2"]) {
      const { refused } = applyOperations(SOURCE, [{ op: "addClass", name } as Operation]);
      expect(refused, `${name} is a usable class name`).toEqual([]);
    }
  });
});
