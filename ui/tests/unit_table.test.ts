/**
 * The unit crosswalk the picker offers (DM-06, DM-59, T-1181).
 *
 * A person picks a unit here, so the table lives here: the model records the choice and every
 * generator reads the model. One path has no person in it — Model Tools inferring a model from
 * a sample (DM-54) — and it keeps the same table in `tools/model-tools/src/infer_schema.py`,
 * in the other repository. Nothing can compare the two from inside one checkout, so each side
 * guards its own shape and both comments name the other; a code added here belongs there too.
 *
 * Both were read out of QUDT's own vocabulary by `qudt:ucumCode` rather than written from
 * memory: `ug/m3` is `MassDensity` to QUDT, not the `MassConcentration` a person would guess.
 */
import { describe, expect, it } from "vitest";
import { UNIT_CODES } from "../src/pages/models/linkml";

describe("the unit crosswalk", () => {
  it("gives every unit an anchor, because a code alone resolves to nothing", () => {
    expect(UNIT_CODES.length).toBeGreaterThan(10);
    for (const entry of UNIT_CODES) {
      expect(entry.qudt, `${entry.code} has no QUDT unit`).toBeTruthy();
      expect(entry.quantityKind, `${entry.code} has no quantity kind`).toBeTruthy();
      expect(entry.ucum, `${entry.code} has no UCUM symbol`).toBeTruthy();
    }
  });

  it("carries local names, not IRIs, because the prefix is declared once in the model", () => {
    for (const entry of UNIT_CODES) {
      for (const value of [entry.qudt, entry.quantityKind]) {
        expect(value).not.toContain("://");
        expect(value).not.toContain(":");
      }
    }
  });

  it("names each code and each UCUM symbol once, so a picker cannot offer two of one unit", () => {
    const codes = UNIT_CODES.map((one) => one.code);
    expect(new Set(codes).size).toBe(codes.length);
    const symbols = UNIT_CODES.map((one) => one.ucum);
    expect(new Set(symbols).size).toBe(symbols.length);
  });
});
