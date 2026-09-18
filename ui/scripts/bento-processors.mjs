#!/usr/bin/env node
// The runner's processor catalog, read from the pinned image and never from a manual (PL-52):
//   docker run --rm ghcr.io/warpstreamlabs/bento:1.21.1 list --format json-full \
//     | node scripts/bento-processors.mjs src/schemas/bento-processors.json \
//         ../../joinedcontext-platform/crates/jc-core/src/kinds/bento_processors.rs
// Writes, per processor a pipeline step may name, its name, the runner's first category and its
// one-line summary. `command` and `subprocess` run programs inside the shared runner and `wasm`
// is the wasm compute kind (PL-34), so none of the three is a step; deprecated ones are left out.
import { readFileSync, writeFileSync } from "node:fs";

const EXCLUDED = new Set(["command", "subprocess", "wasm"]);

const [outPath, rustPath] = process.argv.slice(2);
if (!outPath) {
  console.error("usage: bento list --format json-full | node scripts/bento-processors.mjs <out.json> [<bento_processors.rs>]");
  process.exit(1);
}
const listing = JSON.parse(readFileSync(0, "utf8"));
const catalog = listing.processors
  .filter((processor) => processor.status !== "deprecated" && !EXCLUDED.has(processor.name))
  .map((processor) => ({
    name: processor.name,
    category: (processor.categories ?? [])[0] ?? "Utility",
    summary: (processor.summary ?? "").split("\n")[0].slice(0, 200),
  }))
  .sort((a, b) => a.name.localeCompare(b.name));
writeFileSync(outPath, `${JSON.stringify(catalog, null, 2)}\n`);

if (rustPath) {
  const names = catalog.map((processor) => `    "${processor.name}",`).join("\n");
  writeFileSync(
    rustPath,
    `//! The processors a pipeline step may name (PL-52). Generated from \`bento list --format json-full\`
//! of ghcr.io/warpstreamlabs/bento:v1.21.1 by joinedcontext-portal/ui/scripts/bento-processors.mjs;
//! \`command\`, \`subprocess\` and \`wasm\` are left out on purpose. Change the pin and rerun the
//! script, never edit by hand.

/// Every processor name a \`processor:\` step accepts.
pub const PROCESSORS: &[&str] = &[
${names}
];
`,
  );
}
