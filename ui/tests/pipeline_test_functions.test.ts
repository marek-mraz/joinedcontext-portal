/**
 * What a dropped sample becomes before anybody sees it (T-1135, PL-43, PL-44).
 *
 * These are the functions the test form runs on a file the moment it lands: what format it is,
 * what type it describes, which column is the id, and the Bloblang that maps it. A bug in any of
 * them reaches a person as a mapping that does not compile, so the cases here are the ones a
 * real municipal export actually has — a semicolon delimiter, a Unicode header, a column called
 * `station_id`, a number written as text, an empty file.
 */
import { describe, expect, it } from "vitest";
import {
  draftFromSample,
  errorLines,
  firstRecordOf,
  formatOf,
  idColumnOf,
  spaceOfTarget,
  typeOf,
} from "../src/pages/pipelines/PipelineTest";

describe("the format a sample is in", () => {
  it("takes the file name's word first", () => {
    expect(formatOf("sensors.csv", "{not json at all")).toBe("csv");
    expect(formatOf("sensors.json", "a,b,c")).toBe("json");
    expect(formatOf("SENSORS.CSV", "")).toBe("csv");
  });

  it("reads the first bytes when the name says nothing", () => {
    expect(formatOf("sample", '{"id": 1}')).toBe("json");
    expect(formatOf("sample", "  [1, 2]")).toBe("json");
    expect(formatOf("sample", "id,name\n1,a")).toBe("csv");
    expect(formatOf("sample", "one line, no structure at all")).toBe("csv");
    expect(formatOf("sample", "just text")).toBe("text");
    expect(formatOf("sample", "")).toBe("text");
  });
});

describe("the entity type a file name describes", () => {
  it("is the stem in CamelCase", () => {
    expect(typeOf("air_quality-2026.csv")).toBe("AirQuality2026");
    expect(typeOf("events.json")).toBe("Events");
    expect(typeOf("sample")).toBe("Sample");
  });

  it("never starts with a digit, because an NGSI-LD type may not", () => {
    expect(typeOf("123abc.csv")).toBe("Sample123abc");
    expect(typeOf("2026.csv")).toBe("Sample2026");
    // Nothing left of the name at all is still a type.
    expect(typeOf("---.csv")).toBe("Sample");
  });
});

describe("the column a local id comes from (PL-44)", () => {
  it("prefers a column that says it is one", () => {
    expect(idColumnOf(["id", "name"])).toBe("id");
    expect(idColumnOf(["name", "identifier"])).toBe("identifier");
    expect(idColumnOf(["name", "ID"])).toBe("ID");
  });

  it("then one that ends in an id", () => {
    expect(idColumnOf(["station_id", "name"])).toBe("station_id");
    expect(idColumnOf(["name", "stationId"])).toBe("stationId");
  });

  it("and falls back to the first column, or to `id` when there is none", () => {
    expect(idColumnOf(["x", "y"])).toBe("x");
    expect(idColumnOf([])).toBe("id");
  });
});

describe("the first record of a sample", () => {
  it("pairs a CSV header with the first row", () => {
    expect(firstRecordOf("id,name\n1,Kallio", "csv")).toEqual({ id: "1", name: "Kallio" });
  });

  it("takes a semicolon as the delimiter when there is no comma", () => {
    expect(firstRecordOf("id;name\n1;Kallio", "csv")).toEqual({ id: "1", name: "Kallio" });
  });

  it("unquotes a cell and keeps a Unicode header as it is", () => {
    expect(firstRecordOf('id,"mesto"\n1,"Banská Bystrica"', "csv")).toEqual({
      id: "1",
      mesto: "Banská Bystrica",
    });
  });

  it("gives a header with no row its empty values, and an empty file nothing", () => {
    expect(firstRecordOf("id,name", "csv")).toEqual({ id: "", name: "" });
    expect(firstRecordOf("", "csv")).toBeNull();
    expect(firstRecordOf("\n\n", "csv")).toBeNull();
  });

  it("takes the first element of a JSON array, or the object itself", () => {
    expect(firstRecordOf('[{"id":1},{"id":2}]', "json")).toEqual({ id: 1 });
    expect(firstRecordOf('{"id":1}', "json")).toEqual({ id: 1 });
  });

  it("answers nothing for JSON that is not a record, and for text", () => {
    expect(firstRecordOf("[1,2,3]", "json")).toBeNull();
    expect(firstRecordOf("{not json", "json")).toBeNull();
    expect(firstRecordOf("[]", "json")).toBeNull();
    expect(firstRecordOf("id,name\n1,a", "text")).toBeNull();
  });
});

describe("the mapping a sample drafts (PL-44)", () => {
  const draft = draftFromSample("air_quality.csv", "station_id,pm10,mesto\nBB1,21.5,Banská Bystrica", "ovzdusie");

  it("mints the id under the organization's domain and the target space", () => {
    expect(draft).not.toBeNull();
    expect(draft?.bloblang).toContain('let domain = env("JC_ORG_DOMAIN")');
    expect(draft?.bloblang).toContain('"urn:ngsi-ld:%v:%v:%v:%v".format("AirQuality", $domain, "ovzdusie", this.station_id.string())');
    expect(draft?.type).toBe("AirQuality");
    expect(draft?.idColumn).toBe("station_id");
  });

  it("writes a number as a number and everything else as it came", () => {
    expect(draft?.bloblang).toContain('root.pm10 = { "type": "Property", "value": this.pm10.number().catch(this.pm10) }');
    expect(draft?.bloblang).toContain('root.mesto = { "type": "Property", "value": this.mesto }');
  });

  it("leaves the id column out of the attributes, because it is the id", () => {
    expect(draft?.bloblang).not.toContain("root.station_id =");
  });

  it("quotes a column name Bloblang cannot address bare, and names the attribute legally", () => {
    const awkward = draftFromSample("x.csv", 'id,pm 10 (µg/m³)\n1,21', "ovzdusie");
    // Bloblang's own quoted-path form, `this."name"`, and an attribute name made of the
    // characters a path may hold — the µ and the ³ are not among them.
    expect(awkward?.bloblang).toContain('this."pm 10 (µg/m³)"');
    expect(awkward?.bloblang).toContain("root.pm_10___g_m__ =");
  });

  it("drafts nothing from a file with no record or no column", () => {
    expect(draftFromSample("empty.csv", "", "ovzdusie")).toBeNull();
    expect(draftFromSample("notes.txt", "a line of prose", "ovzdusie")).toBeNull();
  });
});

describe("the space a target endpoint names", () => {
  it("is the fifth segment of the endpoint's URN", () => {
    expect(spaceOfTarget("urn:ngsi-ld:Endpoint:hel.fi:air-quality:ep-all", "helsinki")).toBe("air-quality");
  });

  it("falls back when there is no target, or it is not an Endpoint URN", () => {
    expect(spaceOfTarget(undefined, "helsinki")).toBe("helsinki");
    expect(spaceOfTarget("urn:ngsi-ld:ContextSpace:hel.fi:air-quality:x", "helsinki")).toBe("helsinki");
    expect(spaceOfTarget("urn:ngsi-ld:Endpoint:hel.fi::ep", "helsinki")).toBe("helsinki");
    expect(spaceOfTarget("not-a-urn", "helsinki")).toBe("helsinki");
  });
});

describe("the mapping lines an error names", () => {
  it("is the set of the lines, without the ones that name none", () => {
    expect(
      errorLines({
        errors: [{ line: 3, message: "x" }, { line: 7, message: "y" }, { line: 3, message: "z" }],
      } as never),
    ).toEqual(new Set([3, 7]));
  });

  it("ignores line 0 and a missing line, which mean the error is not on one", () => {
    expect(errorLines({ errors: [{ line: 0, message: "x" }, { message: "y" }] } as never)).toEqual(new Set());
    expect(errorLines({ errors: [] } as never)).toEqual(new Set());
  });
});
