# kit — the dashboard the builder fills in

A React bundle built with the Portal and embedded in its binary. It renders one `spec.json`
(title, sources = entity types and attributes, filters, views: stats, map, table, chart, detail)
over the entities of one endpoint, read as `keyValues`, and nothing else. The Portal's kit pass
asks a model for the specification, applies it and serves this bundle inlined with it as the
preview document (Architecture/19 §1.2, AP-56…AP-60).

`pnpm install && pnpm dev` renders `spec.example.json` against `/api/endpoint/demo/…`, which is
what the demo server stubs. `pnpm test` runs the vitest suites; `pnpm build` writes `dist/kit.js`
and `dist/kit.css`, the two files `src/agents/kit.rs` embeds.
