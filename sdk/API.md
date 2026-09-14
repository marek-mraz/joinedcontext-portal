# @joinedcontext/sdk

### How an app is built
`src/main.tsx` calls `startApp(App, { tokens })`.
Pages use SDK hooks and UI components inside the application shell.
Backend functions live in `functions/{name}.ts` using `@joinedcontext/sdk/server`.
Tests verify behaviors using `@joinedcontext/sdk/testing`.
`design-tokens.json` re-themes all component styling, chart palettes, and map layers.
Allowed imports: relative, `react`, `react-dom/client`, `react/jsx-runtime`, `echarts`, `recharts`, `maplibre-gl`, `@deck.gl/core`, `@deck.gl/layers`, `@deck.gl/aggregation-layers`, `@deck.gl/mapbox`, `@joinedcontext/sdk`, `@joinedcontext/sdk/server`, `vitest`, `@testing-library/react`, `@joinedcontext/sdk/testing`.

---

## Entry Point: `@joinedcontext/sdk`

### Client
```ts
function jc(): Client
```
Returns the active SDK client singleton, reading configuration on first access.
```ts
class ProblemError extends Error
```
RFC 7807 problem error representation containing HTTP status, title, detail, type, and source location.

### Hooks
```ts
function JcProvider({ client, children }: { client?: Client; children: ReactNode }): JSX.Element
```
Context provider supplying an SDK Client to nested hooks and components.
```ts
function useClient(): Client
```
Returns the Client supplied by the nearest JcProvider or the global singleton.
```ts
function useEntities<T extends Row = Row>(type: string, query?: Query, options?: { refreshMs?: number; enabled?: boolean; all?: boolean }): Loaded & { rows: T[] }
```
Loads and polls entities of a given type with query parameters, caching, and race condition protection.
```ts
function useEntity<T extends Row = Row>(id: string | null, attrs?: string[]): Loaded & { entity: T | null }
```
Loads a single entity by NGSI-LD identifier.
```ts
function useSave(): { create: (type: string, attrs: Record<string, Cell>, localId?: string) => Promise<string | null>; update: (id: string, patch: Record<string, Cell>) => Promise<boolean>; remove: (id: string) => Promise<boolean>; saving: boolean; problem: ProblemError | null; clear: () => void }
```
Provides write operations (`create`, `update`, `remove`) that track saving state and ProblemError refusals without throwing.
```ts
function useSchema(type?: string): { schema: Schema | null; typeSchema: TypeSchema | null; error: ProblemError | null }
```
Loads merged JSON Schema definitions for all models exposed by the endpoint.
```ts
function useAccess(): { access: AccessDocument | null; error: ProblemError | null; can: (operation: string, type: string, attr?: string) => Decision }
```
Loads user permissions and provides synchronous evaluation of permissions and prohibitions.
```ts
function useMe(): JcUser | null
```
Returns the identity and roles of the authenticated user from configuration.
```ts
function useFunction<T = unknown>(name: string, body?: unknown, options?: { enabled?: boolean }): Loaded & { data: T | null }
```
Invokes a backend server function and manages its response lifecycle.

### Components
```ts
function AppShell({ title, pages, actions, initial }: { title: string; pages: Page[]; actions?: ReactNode; initial?: string }): JSX.Element
```
Application shell featuring tabbed navigation, user header, error boundaries, and URL hash routing.
```ts
function navigate(id: string): void
```
Programmatically switches the active page tab in the AppShell.
```ts
function Loading({ label }: { label?: string }): JSX.Element
```
Accessible loading indicator with polite ARIA announcement.
```ts
function Empty({ children }: { children?: ReactNode }): JSX.Element
```
Placeholder text displayed when data sets or search results are empty.
```ts
function Problem({ error, onRetry }: { error: ProblemError | Error | null | undefined; onRetry?: () => void }): JSX.Element | null
```
Alert banner rendering title, detail, and optional retry action for a ProblemError or Error.
```ts
class ErrorBoundary extends Component<{ children?: ReactNode; fallback?: (error: Error, reset: () => void) => ReactNode }, { error: Error | null }>
```
React error boundary that catches rendering exceptions, reports them to the host window, and displays fallback UI.
```ts
function reportError(error: unknown): void
```
Logs an error and posts a `jc-error` message with stack information to the parent window if framed.
```ts
function StatTiles({ rows, tiles, loading }: { rows: Row[]; tiles: StatTile[]; loading?: boolean }): JSX.Element
```
Renders a grid of aggregated numeric summary cards.
```ts
function EntityTable<T extends Row = Row>(props: { rows: T[]; columns?: Array<string | ColumnDef<T>>; pageSize?: number; selected?: string | null; onSelect?: (row: T) => void; initialSort?: { attr: string; dir: "asc" | "desc" }; loading?: boolean; error?: ProblemError | Error | null; empty?: ReactNode; caption?: string }): JSX.Element
```
Sortable, paged tabular display with accessible keyboard selection.
```ts
function defaultColumns(rows: Row[], max?: number): string[]
```
Infers sensible non-geometry attribute columns from entity rows.
```ts
function EntityDetail({ row, attrs, title, onClose }: { row: Row | null | undefined; attrs?: string[]; title?: string; onClose?: () => void }): JSX.Element
```
Definition list detailing every attribute of a selected entity.
```ts
function FilterBar({ children, onReset, shown, total }: { children?: ReactNode; onReset?: () => void; shown?: number; total?: number }): JSX.Element
```
Toolbar container for search, select, range, and date-range filter widgets.
```ts
function SearchBox({ binding, placeholder }: { binding: FilterBinding; placeholder?: string }): JSX.Element
```
Text search input bound to one or more entity attributes.
```ts
function SelectFilter({ binding, allLabel }: { binding: FilterBinding; allLabel?: string }): JSX.Element
```
Dropdown select filter populated with distinct column values.
```ts
function RangeFilter({ binding }: { binding: FilterBinding }): JSX.Element
```
Paired number inputs defining a numeric minimum and maximum filter bound.
```ts
function DateRangeFilter({ binding }: { binding: FilterBinding }): JSX.Element
```
Paired date inputs defining a start date and full-day end timestamp filter.
```ts
function EntityForm({ type, row, fields, rows, title, onSaved, onCancel }: { type: string; row?: Row | null; fields?: string[]; rows?: Row[]; title?: string; onSaved?: (id: string) => void; onCancel?: () => void }): JSX.Element
```
Schema-driven entity creation and editing form that checks permissions and displays inline errors.
```ts
function parseInput(field: Field, text: string): { value: Cell } | { error: string }
```
Parses raw text input into a typed Cell value according to field schema rules.
```ts
function ExportButton({ rows, columns, filename, formats, location, title, canvas }: { rows: Row[]; columns?: string[]; filename?: string; formats?: ExportFormat[]; location?: string; title?: string; canvas?: () => HTMLCanvasElement | null }): JSX.Element
```
Export actions for downloading data as CSV, GeoJSON, PDF, or canvas PNG.
```ts
function ChartCard({ title, option, height, loading, error, empty, onSelect, onReady }: { title?: string; option: Record<string, unknown> | null; height?: number; loading?: boolean; error?: ProblemError | Error | null; empty?: ReactNode; onSelect?: (name: string) => void; onReady?: (chart: echarts.ECharts) => void }): JSX.Element
```
ECharts container card with automatic resizing and item click callbacks.
```ts
function BarChartCard(props: ChartSpec & { rows: Row[]; title?: string; horizontal?: boolean; height?: number; loading?: boolean; error?: ProblemError | Error | null; onSelect?: (name: string) => void }): JSX.Element
```
Card displaying an aggregated bar chart.
```ts
function LineChartCard(props: ChartSpec & { rows: Row[]; title?: string; height?: number; loading?: boolean; error?: ProblemError | Error | null }): JSX.Element
```
Card displaying a sorted line chart.
```ts
function PieChartCard(props: ChartSpec & { rows: Row[]; title?: string; height?: number; loading?: boolean; error?: ProblemError | Error | null; onSelect?: (name: string) => void }): JSX.Element
```
Card displaying a proportion pie chart with an automatic "Other" slice.
```ts
function TimeSeriesCard(props: ({ rows: Row[]; time: string; y?: string; agg?: Agg; bucket?: Bucket } | { series: TemporalRow[]; attr: string }) & { title?: string; height?: number; loading?: boolean; error?: ProblemError | Error | null }): JSX.Element
```
Card displaying time-series trends from either rows or temporal entity series.
```ts
function barOption(rows: Row[], spec: ChartSpec & { horizontal?: boolean }, tokens?: DesignTokens): Record<string, unknown>
function lineOption(rows: Row[], spec: ChartSpec, tokens?: DesignTokens): Record<string, unknown>
function pieOption(rows: Row[], spec: ChartSpec, tokens?: DesignTokens): Record<string, unknown>
function timeSeriesOption(input: { rows: Row[]; time: string; y?: string; agg?: Agg; bucket?: Bucket } | { series: TemporalRow[]; attr: string }, tokens?: DesignTokens): Record<string, unknown>
```
The ECharts options the chart cards draw; pass one, changed, to `ChartCard` for a chart the cards do not cover.
```ts
function bucketOf(iso: string, bucket: Bucket): string | null
```
Truncates an ISO time to the UTC hour, day, ISO week (Monday) or month; null when unparseable.
```ts
function EntityMap({ rows, location, label, color, selected, onSelect, basemap, mode, height, radius }: { rows: Row[]; location?: string; label?: string; color?: string; selected?: string | null; onSelect?: (row: Row) => void; basemap?: string; mode?: MapMode; height?: number; radius?: number }): JSX.Element
```
MapLibre geospatial map supporting deck.gl scatter, hexbin, and grid aggregations.
```ts
const DECK_THRESHOLD: 50000
function renderPath(count: number, mode: MapMode): "maplibre" | "deck-points" | "deck-hexbin" | "deck-grid"
```
How `EntityMap` draws: MapLibre below `DECK_THRESHOLD` rows, a deck.gl overlay from it and for `hexbin`/`grid`.
```ts
function colorRamp(value: Cell, range: [number, number] | null, tokens?: DesignTokens): string
```
The map colour of a number between the tokens' `map.low` and `map.high`; `map.point` when not a number.

### Filters Logic
```ts
function filterRows<T extends Row>(rows: T[], filters: FilterDef[], values: FilterValue[]): T[]
```
Filters an in-memory row array against an active set of filter definitions and values.
```ts
function useFilters<T extends Row>(rows: T[], filters: FilterDef[]): { shown: T[]; values: FilterValue[]; set: (index: number, value: FilterValue) => void; bind: (index: number) => FilterBinding; reset: () => void }
```
Manages filter state, returning filtered rows and two-way control bindings.

### Helpers
```ts
function aggregate(rows: Row[], agg: Agg, attr?: string): number | null
```
Calculates count, sum, avg, min, or max over numeric attributes.
```ts
function columnKind(rows: Row[], attr: string): Column
```
Infers whether an attribute contains numbers, dates, geometry objects, or text.
```ts
function distinct(rows: Row[], attr: string): string[]
```
Returns a sorted list of unique non-empty string values for an attribute.
```ts
function extent(rows: Row[], attr: string): [number, number] | null
```
Returns the minimum and maximum numeric values found in an attribute.
```ts
function format(value: Cell, kind?: Column): string
```
Formats a cell value for human-readable display.
```ts
function pointOf(value: Cell): [number, number] | null
```
Extracts `[longitude, latitude]` coordinates from a GeoProperty.
```ts
function groupBy(rows: Row[], attr: string, agg?: Agg, valueAttr?: string, top?: number): Group[]
```
Groups rows by attribute key and computes an aggregated value for each group.
```ts
function toFeatureCollection(rows: Row[], location?: string, properties?: string[]): { type: "FeatureCollection"; features: Array<{ type: "Feature"; id: string; geometry: Geo; properties: Record<string, string | number | boolean | null> }> }
```
Converts entity rows with geometry into a GeoJSON FeatureCollection.

### Tokens
```ts
const DEFAULT_TOKENS: DesignTokens
```
Standard design token values for palette, fonts, spacing, radius, and maps.
```ts
function resolveTokens(input: unknown): { tokens: DesignTokens; warnings: string[] }
```
Validates and merges user design tokens against defaults.
```ts
function applyTokens(input: unknown, root?: HTMLElement): DesignTokens
```
Applies design tokens as CSS variables to the document or target element.
```ts
function currentTokens(): DesignTokens
```
Returns the active design tokens.
```ts
function echartsTheme(tokens?: DesignTokens): Record<string, unknown>
```
Generates an Apache ECharts theme configuration from design tokens.
```ts
function rechartsPalette(tokens?: DesignTokens): string[]
```
Returns the chart color palette array for Recharts.
```ts
function mapColors(tokens?: DesignTokens): DesignTokens["map"]
```
Returns map styling colors for points, highlights, ramps, and strokes.

### Start
```ts
function startApp(App: ComponentType, options?: { tokens?: unknown; root?: HTMLElement; doc?: Document }): { client: Client; tokens: DesignTokens }
```
Bootstraps a generated application by reading configuration, applying tokens, and mounting React.

### Types
- `Row`: Entity object `{ id: string, type: string } & Record<string, Cell>`.
- `Cell`: Scalar value `string | number | boolean | Geo | null`.
- `Geo`: Geometry object `{ type: string, coordinates: unknown }`.
- `Column`: Kind `"number" | "date" | "geo" | "text"`.
- `Agg`: Aggregation `"count" | "sum" | "avg" | "min" | "max"`.
- `Group`: Aggregated group `{ key: string, value: number }`.
- `Page`: Navigation tab `{ id: string, label: string, render: () => ReactNode }`.
- `StatTile`: Summary metric `{ label: string, agg?: Agg, attr?: string, unit?: string, digits?: number }`.
- `ColumnDef<T>`: Table column `{ attr: string, label?: string, render?: (row: T) => ReactNode }`.
- `ExportFormat`: File format `"csv" | "geojson" | "pdf" | "png"`.
- `ChartSpec`: Metric spec `{ x: string, y?: string, agg?: Agg, top?: number }`.
- `Bucket`: Time bucket `"hour" | "day" | "week" | "month"`.
- `MapMode`: Map mode `"auto" | "points" | "hexbin" | "grid"`.

---

## Entry Point: `@joinedcontext/sdk/server`

```ts
interface FnRequest { method: "GET" | "POST"; query: Record<string, string>; body: any; user: JcUser | null }
```
Incoming HTTP request payload passed to backend server functions.
```ts
interface FnResponse { status?: number; body?: unknown }
```
Response status and body returned by backend server functions.
```ts
interface FnContext { jc: DataClient; log(...parts: unknown[]): void }
```
Context providing data access and logging to server functions.
```ts
type FnHandler = (request: FnRequest, ctx: FnContext) => Promise<FnResponse>
```
Standard handler signature implemented by backend function files.

---

## Entry Point: `@joinedcontext/sdk/testing`

```ts
function stubTransport(fixture?: Fixture): StubTransport
```
Creates an in-memory transport holding entity fixtures and recording request calls.
```ts
function stubClient(fixture?: Fixture, config?: Partial<JcConfig>): Client & { transport: StubTransport }
```
Constructs an SDK Client wired to stub transport for unit tests.
```ts
function fakeContext(fixture?: Fixture & { user?: JcUser | null }): { jc: DataClient; log(...parts: unknown[]): void; logs: unknown[][] }
```
Constructs a server function execution context capturing log statements.

---

## Rows (SDK-03)
Entities read through the SDK are flattened into `Row` objects. `id` (URN) and `type` are always present. KeyValues attributes are simplified into `Cell` values: `Property` becomes primitive value, `GeoProperty` becomes a GeoJSON object, and `LanguageProperty` is resolved to the preferred language string.

## Errors (SDK-04)
Refusals from endpoints and policy checks arrive as `ProblemError` instances containing HTTP `status`, `title`, and `detail`. The `useSave` hook exposes `problem` directly without throwing exceptions, allowing `EntityForm` and `<Problem error={problem} />` to render error details inline without reloading the page.

## Access
Permissions are evaluated using `useAccess().can(operation, type, attr)`. Recognized operation names include:
- `queryEntity`: Query entities of a type.
- `retrieveEntity`: Fetch a single entity by ID.
- `queryTemporal`: Query temporal history.
- `createEntity`: Create a new entity instance.
- `updateAttrs`: Update selected attributes of an entity.
- `updateEntity`: Replace an existing entity.
- `deleteEntity`: Delete an entity.

---

## Example: Complete Page

```tsx
import { useState } from "react";
import {
  useEntities, useFilters, useAccess,
  FilterBar, SearchBox, SelectFilter, StatTiles,
  BarChartCard, EntityTable, EntityMap, EntityDetail, EntityForm
} from "@joinedcontext/sdk";

export function StationsPage() {
  const { rows, loading, error } = useEntities("BikeHireDockingStation");
  const { shown, bind, reset } = useFilters(rows, [
    { kind: "search", attrs: ["name"] },
    { kind: "select", attr: "status" },
  ]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const access = useAccess();
  const selectedRow = shown.find((r) => r.id === selectedId) ?? null;

  return (
    <div>
      <FilterBar shown={shown.length} total={rows.length} onReset={reset}>
        <SearchBox binding={bind(0)} />
        <SelectFilter binding={bind(1)} />
      </FilterBar>
      <StatTiles
        rows={shown}
        loading={loading}
        tiles={[
          { label: "Stations", agg: "count" },
          { label: "Available Bikes", agg: "sum", attr: "availableBikeNumber" },
        ]}
      />
      <BarChartCard rows={shown} x="name" y="availableBikeNumber" top={10} title="Top Stations" />
      <EntityMap rows={shown} label="name" selected={selectedId} onSelect={(r) => setSelectedId(r.id)} />
      <EntityTable rows={shown} error={error} loading={loading} selected={selectedId} onSelect={(r) => setSelectedId(r.id)} />
      <EntityDetail row={selectedRow} onClose={() => setSelectedId(null)} />
      {access.can("updateAttrs", "BikeHireDockingStation").ok && selectedRow && (
        <button type="button" onClick={() => setEditing(true)}>Edit</button>
      )}
      {editing && selectedRow && (
        <EntityForm
          type="BikeHireDockingStation"
          row={selectedRow}
          onSaved={() => setEditing(false)}
          onCancel={() => setEditing(false)}
        />
      )}
    </div>
  );
}
```

---

## Example: Backend Function & Test

```ts
// functions/rebalance.ts: the default export is the handler (SDK-21)
import type { FnHandler } from "@joinedcontext/sdk/server";

const rebalance: FnHandler = async (req, ctx) => {
  if (req.method !== "POST") return { status: 405, body: { title: "POST only" } };
  const { stationId, bikes } = req.body ?? {};
  if (typeof stationId !== "string" || typeof bikes !== "number") return { status: 400, body: { title: "stationId and bikes are required" } };
  ctx.log("Rebalancing station", stationId, bikes);
  await ctx.jc.entities.update(stationId, { availableBikeNumber: bikes });
  return { status: 200, body: { ok: true, stationId } };
};
export default rebalance;
```

```ts
// functions/rebalance.test.ts
import { describe, expect, it } from "vitest";
import { fakeContext } from "@joinedcontext/sdk/testing";
import rebalance from "./rebalance";

describe("rebalance", () => {
  it("updates the station's bike count and logs it", async () => {
    const ctx = fakeContext({
      entities: [{ id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", availableBikeNumber: 2 }],
    });
    const res = await rebalance(
      { method: "POST", query: {}, body: { stationId: "urn:ngsi-ld:Station:example.org:demo:1", bikes: 8 }, user: null },
      ctx,
    );
    expect(res.status).toBe(200);
    expect(ctx.logs[0]).toEqual(["Rebalancing station", "urn:ngsi-ld:Station:example.org:demo:1", 8]);
    const updated = await ctx.jc.entities.get("urn:ngsi-ld:Station:example.org:demo:1");
    expect(updated.availableBikeNumber).toBe(8);
  });
});
```
