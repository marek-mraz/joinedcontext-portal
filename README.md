# joinedcontext-portal

The Portal is **one application** that manages the whole platform: Rust backend and React UI in one repository, one image, one route (`/` and `/api/v1/`, resource collections included).

```
joinedcontext-portal/
├── Cargo.toml          Rust application (axum): Portal API, preferences, ServiceAccounts and API keys,
├── src/                resource API, reconciler (jcctl crate as library) and MCP config surface;
│                       serves the built UI from ui/dist at /
└── ui/                 Vite + React + TypeScript: Portal UI, LinkML editor, dashboards (MapLibre + deck.gl),
                        Apps kit, i18n (sk/en/de/cs); API client generated from the backend's OpenAPI document
```

Specification: `../docs/Architecture/09-portal.md` (Portal), `06-configuration-as-code.md` (reconciler),
`11-data-models.md` (LinkML editor), `12-identity-and-access.md` (ServiceAccounts), `16-apps-on-demand.md`;
requirements UI-01…UI-26, MF-01…MF-34, PF-34…PF-40.

Build: `npm ci && npm run build` in `ui/`, then `cargo build --release`; the binary embeds `ui/dist`.
