import { describe, expect, it } from "vitest";
import { localized } from "../src/api/manifest";

describe("a manifest title (UI-50)", () => {
  it("reads a plain string as it is, whatever the locale", () => {
    expect(localized("City bikes", "sk", "city-bikes")).toBe("City bikes");
  });

  it("falls back to the name for an empty or missing title", () => {
    expect(localized("  ", "en", "city-bikes")).toBe("city-bikes");
    expect(localized(undefined, "en", "city-bikes")).toBe("city-bikes");
  });

  it("still reads the legacy language map", () => {
    expect(localized({ sk: "Bicykle", en: "Bikes" }, "sk-SK", "bikes")).toBe("Bicykle");
    expect(localized({ de: "Räder" }, "cs", "bikes")).toBe("bikes");
  });
});
