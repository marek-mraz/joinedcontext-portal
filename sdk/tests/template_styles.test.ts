/**
 * Which stylesheet owns which class in a generated application (T-1060, AP-56, SDK-01).
 *
 * A generated app loads three, and they are split by who may edit them:
 *
 * | file | holds | whose |
 * |---|---|---|
 * | `@joinedcontext/sdk/style.css` | MapLibre and the typeface | the SDK's, replaced on upgrade |
 * | `src/components/components.css` | the shipped components' `.jc-*` classes | the SDK's |
 * | `src/app.css` | the application's own `.app-*` page layout | the author's |
 *
 * The Portal's own `ui/src/index.css` is in none of them: an application is not the Portal and
 * never loads its stylesheet. This test walks the files so a de-slop that proposes folding one
 * into another has to say which of the three owners it is moving work to.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const TEMPLATE = join(__dirname, "..", "template", "src");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      return sources(path);
    }
    return path.endsWith(".tsx") && !path.endsWith(".test.tsx") ? [path] : [];
  });
}

/** Every `className="…"` word of the template's own pages and components. */
function classesUsed(): Set<string> {
  const used = new Set<string>();
  for (const path of sources(TEMPLATE)) {
    const text = readFileSync(path, "utf8");
    for (const match of text.matchAll(/className=(?:"([^"]*)"|\{`([^`]*)`\})/g)) {
      for (const name of (match[1] ?? match[2] ?? "").split(/\s+/)) {
        const clean = name.replace(/\$\{[^}]*\}/g, "").trim();
        if (clean) {
          used.add(clean);
        }
      }
    }
  }
  return used;
}

function classesDefined(file: string): Set<string> {
  const text = readFileSync(join(TEMPLATE, file), "utf8");
  return new Set([...text.matchAll(/\.([a-zA-Z][\w-]*)/g)].map((match) => match[1]));
}

describe("the stylesheets a generated application loads", () => {
  it("defines every app-level class its own pages use", () => {
    const own = classesDefined("app.css");
    const missing = [...classesUsed()].filter(
      (name) => name.startsWith("app-") && !own.has(name),
    );
    expect(missing).toEqual([]);
    // And it is not an empty file kept for its name: the pages do use it.
    expect([...classesUsed()].some((name) => name.startsWith("app-"))).toBe(true);
  });

  it("defines every component class the shipped components use", () => {
    const shipped = classesDefined(join("components", "components.css"));
    const missing = [...classesUsed()].filter(
      (name) => name.startsWith("jc-") && !shipped.has(name),
    );
    expect(missing).toEqual([]);
  });

  it("keeps the two files to their own halves", () => {
    const own = classesDefined("app.css");
    const shipped = classesDefined(join("components", "components.css"));
    expect([...own].filter((name) => name.startsWith("jc-"))).toEqual([]);
    expect([...shipped].filter((name) => name.startsWith("app-"))).toEqual([]);
  });

  it("loads the SDK's stylesheet and its own two, and never the Portal's", () => {
    const main = readFileSync(join(TEMPLATE, "main.tsx"), "utf8");
    expect(main).toContain('import "@joinedcontext/sdk/style.css"');
    expect(main).toContain('import "./components/components.css"');
    expect(main).toContain('import "./app.css"');
    expect(main).not.toContain("index.css");
  });
});
