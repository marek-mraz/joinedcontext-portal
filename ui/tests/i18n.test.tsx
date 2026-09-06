import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import sk from "../src/locales/sk.json";
import en from "../src/locales/en.json";
import de from "../src/locales/de.json";
import cs from "../src/locales/cs.json";

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  let keys: string[] = [];
  for (const [key, value] of Object.entries(obj)) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      keys = keys.concat(flattenKeys(value as Record<string, unknown>, fullKey));
    } else {
      keys.push(fullKey);
    }
  }
  return keys.sort();
}

describe("i18n", () => {
  it("has identical flattened key sets across all four bundles", () => {
    const skKeys = flattenKeys(sk);
    const enKeys = flattenKeys(en);
    const deKeys = flattenKeys(de);
    const csKeys = flattenKeys(cs);

    expect(skKeys.length).toBeGreaterThan(0);
    expect(enKeys).toEqual(skKeys);
    expect(deKeys).toEqual(skKeys);
    expect(csKeys).toEqual(skKeys);
  });

  it("gives the German string for app.title after changeLanguage('de')", async () => {
    await i18n.changeLanguage("de");
    expect(i18n.t("app.title")).toBe(de.app.title);
  });

  it("falls back to Slovak for an unknown language", async () => {
    await i18n.changeLanguage("unknown-lang");
    expect(i18n.t("form.submit")).toBe(sk.form.submit);
  });

  it("formats ICU plural of form.errors in Slovak for counts 0, 1, 2, and 5", async () => {
    await i18n.changeLanguage("sk");
    expect(i18n.t("form.errors", { count: 0 })).toBe("Bez chýb");
    expect(i18n.t("form.errors", { count: 1 })).toBe("1 chyba");
    expect(i18n.t("form.errors", { count: 2 })).toBe("2 chyby");
    expect(i18n.t("form.errors", { count: 5 })).toBe("5 chýb");
  });

  it("keeps document.documentElement.lang in step with the locale", async () => {
    await i18n.changeLanguage("de");
    expect(document.documentElement.lang).toBe("de");
    await i18n.changeLanguage("sk");
    expect(document.documentElement.lang).toBe("sk");
  });

  it("interpolates {name} in auth.signedInAs", async () => {
    await i18n.changeLanguage("sk");
    expect(i18n.t("auth.signedInAs", { name: "Alice" })).toBe("Prihlásený ako Alice");

    await i18n.changeLanguage("en");
    expect(i18n.t("auth.signedInAs", { name: "Alice" })).toBe("Signed in as Alice");
  });
});
