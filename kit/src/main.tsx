import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { parseSpec } from "./spec";
import "./index.css";

/**
 * The Portal inlines `{"slug": …, "spec": …}` into `#kit-spec` when it serves the preview
 * document (Architecture/19 §1.2); `index.html` carries an example for `vite dev`.
 */
const element = document.getElementById("kit-spec");
const root = createRoot(document.getElementById("root")!);
let payload: { slug?: unknown; spec?: unknown } = {};
try {
  payload = JSON.parse(element?.textContent ?? "{}") as typeof payload;
} catch {
  payload = {};
}
const parsed = parseSpec(payload.spec);
if (parsed.spec && typeof payload.slug === "string" && payload.slug !== "") {
  root.render(
    <StrictMode>
      <App slug={payload.slug} spec={parsed.spec} />
    </StrictMode>,
  );
} else {
  root.render(
    <div className="app">
      <header><h1>This dashboard cannot be shown</h1></header>
      <ul className="error-list">
        {(parsed.errors.length ? parsed.errors : ["slug: missing"]).map((error) => (
          <li key={error}>{error}</li>
        ))}
      </ul>
    </div>,
  );
}
