/**
 * The page the assistant opened, in words (T-0746, UI-45): a section's title and the resource the
 * route names, never the address or its query.
 */
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { pageOf } from "../src/assistant/pageOf";

const t = (key: string, values?: Record<string, string>) => i18n.t(key, values);

describe("the page the assistant opened", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names a section and the resource the route hands over", () => {
    expect(pageOf("/projects/helsinki/endpoints", t)).toBe("Endpoints");
    expect(pageOf("/projects/helsinki/pipelines?edit=hel-news&draft=hel-news", t)).toBe("Pipelines: hel-news");
    expect(pageOf("/projects/helsinki/access?grant=jana-kovacova-steward-helsinki", t)).toBe(
      "Access: jana-kovacova-steward-helsinki",
    );
    expect(pageOf("/projects/helsinki/spaces/complete?space=city-bikes", t)).toBe("Complete this space: city-bikes");
    expect(pageOf("/projects/helsinki/spaces/city-bikes", t)).toBe("Context Spaces: city-bikes");
    expect(pageOf("/projects/helsinki/models", t)).toBe("Data models");
    expect(pageOf("/endpoints", t)).toBe("All endpoints");
  });

  it("says another page for a route it does not know, and never shows the address", () => {
    for (const route of ["/playground?x=1", "/projects/helsinki/unknown?edit=x", "/"]) {
      const said = pageOf(route, t);
      // Read inside the notice's "Opened {page}".
      expect(said).toBe("another page");
      expect(said).not.toContain("/");
    }
  });
});
