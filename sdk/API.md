# @joinedcontext/sdk

### How an app is built
`src/main.tsx` calls `startApp(App, { tokens })`.
Pages compose the template's components with this package's hooks. The components are files of the application in `src/components/` (`AppShell`, `states` with `Problem`, `Loading`, `Empty` and `ErrorBoundary`, `StatTiles`, `EntityTable`, `EntityDetail`, `filters`, `EntityForm`, `ExportButton`, `charts`, `EntityMap`, `components.css`): read them in the pack and change them like any other file. This reference covers the package they stand on.
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
function useAccess(endpoint?: string): { access: AccessDocument | null; error: ProblemError | null; can: (operation: string, type: string, attr?: string) => Decision }
```
Loads user permissions (of the named endpoint, the primary by default) and provides synchronous evaluation of permissions and prohibitions.
```ts
function useMe(): JcUser | null
```
Returns the identity and roles of the authenticated user from configuration.
```ts
function useFunction<T = unknown>(name: string, body?: unknown, options?: { enabled?: boolean }): Loaded & { data: T | null }
```
Invokes a backend server function and manages its response lifecycle.

### Tables, forms, maps, exports
```ts
function compare(a: Row, b: Row, attr: string, dir: "asc" | "desc"): number
```
Orders two rows by one attribute: numbers as numbers, text in the document's locale, empty last.
```ts
function fieldOf(name: string, schema: TypeSchema | undefined, kind: Column): Field
```
The form input for one attribute from the endpoint's JSON Schema: number, text, select, date, checkbox or geo.
```ts
const NO_BASEMAP: string
function styleFor(basemap?: string): string | StyleSpecification
```
The MapLibre style: the configured basemap URL, or a plain background and `NO_BASEMAP` shown beside it.
```ts
const NO_LOCATIONS: string
```
What a map shows instead of a count when none of its rows has a location: read the location from the endpoint that carries it.
```ts
function mapWorkerReady(doc?: Document): Promise<boolean>
```
Resolves once MapLibre's worker is set up for the preview frame; create a map only after it.
```ts
function toCsv(columns: string[], rows: Row[]): Blob
function toGeoJson(rows: Row[], locationAttr: string): GeoJsonCollection
function toPdf(input: { title: string; endpoint: string; filters: string; takenAt: string; attribution?: string; lines: string[] }): Blob
function toPng(canvas: HTMLCanvasElement): Promise<Blob>
function download(blob: Blob, filename: string): void
```
Exports of what is shown (AP-66) and the browser download of the result.

### Entity grid (UI-64, UI-71, SDK-29)
```ts
function EntityGrid(props: EntityGridProps): JSX.Element
function useEntityGrid(options: UseEntityGridOptions): { rows, columns, total, loading, error, state, labels, cellOf, toggleMeta, setOffset, setSort, moveActive, getGridProps, getHeaderProps, getRowProps, getCellProps, reload }
function parseGridConfig(raw: unknown): { config?: ResolvedGridConfig; findings: ConfigFinding[] }
const gridConfigSchema: JSONSchema // published as grid.config.schema.json
const DEFAULT_PAGE_SIZE = 50, MAX_PAGE_SIZE = 1000
const DEFAULT_LABELS: GridLabels

// The filter row as a query (T-1429): the endpoint filters, never the browser
function queryFromFilters(columns: FilterColumn[], filters: Record<string, ColumnFilter>): { q?: string; idPattern?: string }
function opsForKind(kind: FilterKind): FilterOp[]
function andQ(...parts: (string | undefined)[]): string | undefined

// Cells corrected by hand, applied through the source's own `patch` (T-1430; UI-67, AG-78)
function applyChanges(options: { source, entities, observed?, fallback?, now? }): Promise<ApplyResult>
function attrsBody(changes: AttributeChange[], observed: Observed, now?: () => string): Record<string, unknown>
const MAX_ENTITIES = 50

// One attribute's history (T-1431): the temporal read as a table and a line
function EntityHistory(props: EntityHistoryProps): JSX.Element
function asCsv(points: HistoryPoint[], attr: string): string
const MAX_POINTS = 1000

// The space beside an endpoint (T-1435): the same page of ids through both doors
function EntityCompare(props: EntityCompareProps): JSX.Element
function pageSource(rows: RichRow[]): EntitySource
const DEFAULT_COMPARE_LABELS: CompareLabels
```
One spreadsheet-like grid of one entity type: an id column pinned, a value with its unit per attribute, and per attribute a menu that adds `observedAt`, unit, `datasetId`, `createdAt`, `modifiedAt` columns. Arrow keys, Home/End and PageUp/PageDown move the active cell (`role="grid"`). The config is data (`{ source, type, columns, entityTimestamps, filters, pageSize, mode, editableAttrs, history, compareWith, density, rowActions }`); `parseGridConfig` refuses unknown keys with JSON paths and fills defaults. Props: `config`, `source`, `labels` (every visible string; English defaults), `state` + `onStateChange` (controlled per key, else uncontrolled), `renderers` by attribute name, cell kind or `id` for the pinned identifier column, `onOpenRelationship`, `onRows` (the page of rows and the offset it starts at, for a host that exports what is shown), `toolbar`, `empty`, `classNames`. `useEntityGrid` is the same behaviour without markup. Values render as text; sorting orders the loaded page only, which the header says.

The header's second row filters at the endpoint (UI-66, EP-07): per column the operators its content allows (contains/is/is not/is empty/has a value for text, comparisons and between for numbers and dates, the id by pattern, a metadata column on `attr.observedAt`), composed into one `q` joined by `;` with every value quoted and every pattern's metacharacters escaped, shown under the grid with a copy button and an "edit as text" switch for a `q` the rows cannot show. The footer carries the endpoint's own `NGSILD-Results-Count`, and nothing when a narrowed answer carried none (R22). In `mode: "edit"` the columns of `editableAttrs` take a typed value — the value itself, not the text, so a unit is kept — which is marked, counted, reviewed as a batch with one `observedAt` choice, and applied one `PATCH …/attrs` per entity through the source's `patch`; a refused entity keeps its cell and the endpoint's own sentence. A source without `patch` has no edit mode and one without `history` offers no history, which is how a read-only surface switches both off.

`EntityCompare` puts a space and an endpoint side by side (UI-69, EP-61): the left side is read first and the right side is asked for exactly those ids (`id=`, split by `idChunks` so no query string passes 3 500 characters), so the two pages hold the same entities. `marks` paints what the right side does not answer on the left — a row it leaves out is struck through, an attribute it does not carry is shaded — and nothing is ever drawn on the right about a value it withheld. Both sides are view mode, both read with the person's own session, and a space the person may not read leaves the endpoint side alone with the reason. Under 900 px the two stack.
```ts
function endpointSource(slug: string, transport: Transport, language?: string): EntitySource
function spaceSource(space: string, transport: Transport, language?: string): EntitySource
function sourceFor(source: { kind: "endpoint"; slug } | { kind: "space"; space }, transport: Transport, language?: string): EntitySource
function fixtureSource(entities: object[], language?: string): EntitySource
function idChunks(ids: string[], maxChars?: number): string[][]
function transportFor(config: JcConfig): Transport // e.g. transportFor(jc().config)
function originTransport(fetchImpl?: typeof fetch, doc?: Document): Transport // a host page's own: same origin, the person's session and CSRF header
class SourceError extends Error { status: number }
function historyOf(body: unknown, attr: string): HistoryPoint[]
```
Where a grid reads: `query`, `get`, and where the source offers them `history`, `patch`, `remove`. The two built-in sources read the normalized representation with `options=sysAttrs` through the person's session; a space's 404 says "no grant on this space" whether or not it exists. The Endpoint's Policy decides every write.
```ts
function toRichRow(entity: object, language?: string): RichRow // { id, type, createdAt, modifiedAt, scope, cells, raw }
function toRichCell(attr: unknown, language: string): RichCell | null // { kind, value, unitCode, observedAt, datasetId, createdAt, modifiedAt, object, geometryType, languageMap, sub }
function projectRow(rich: RichRow, language?: string): Row
function attributesOf(rows: RichRow[]): string[]
function cellText(cell: RichCell | RichCell[] | undefined): string
```
The rich cell model: a multi-instance attribute (several `datasetId`s) is an array; `raw` is the entity as received, so an edit never drops a member the grid does not show.

### Geometry: view and edit (UI-72, SDK-29)
```ts
function GeoView(props: { value, selectedId?, onSelect?, accent?, basemap?, label? }): JSX.Element
function GeoEditor(props: { value, onChange, allowed?, snapTo?, basemap?, engine?, label? }): JSX.Element
function checkGeometry(raw: unknown, allowed?: GeometryType[]): { geometry: Geometry | null; findings: GeoFinding[] }
function geometryOf(raw: unknown, allowed?: GeometryType[]): { geometry: Geometry | null; findings: GeoFinding[] }
function rowsOf(g: Geometry | null): CoordinateRow[]            // the coordinate table's rows
function withPosition(g: Geometry, at: number[], p: Position): Geometry
function withoutPosition(g: Geometry, at: number[]): Geometry
function rowLabel(at: number[]): string                          // "Point 1.3"
function modesFor(allowed: GeometryType[]): DrawMode[]           // the toolbar
function positionsOf(coordinates: unknown): Position[]
function boundsOf(features: GeoFeature[]): [[number, number], [number, number]] | null
function featuresOf(value): GeoFeature[]
function terraDrawEngine(): Promise<DrawEngine>                  // the default, loaded on map load
const GEOMETRY_TYPES = ["Point","LineString","Polygon","MultiPoint","MultiLineString","MultiPolygon"]
const MAX_VERTICES = 10000
const DRAW_MODES: DrawMode[]
```
`GeoView` draws a geometry, a Feature or a list of them on the kit's base map and fits to it once;
`selectedId` thickens one shape and `onSelect` reports a click. `GeoEditor` is the same map with a
toolbar (only the modes `allowed` permits, plus select, undo, redo and delete) **and the same
geometry as a table of longitude/latitude inputs**, which is the keyboard path: every action of the
map is possible from the table. A paste or an uploaded `.geojson` goes through `geometryOf`, which
also takes a `Feature` or a `FeatureCollection` (one type becomes the `Multi-` form where `allowed`
has it). Nothing reaches `onChange` unchecked: `checkGeometry` closes an open ring, normalises the
winding, keeps altitudes, and refuses a bow tie, a hole outside its shell, a pair out of ±180/±90
(saying so when the two look swapped) and more than `MAX_VERTICES` points — the findings appear on
screen instead. Output is RFC 7946, which is what an NGSI-LD GeoProperty's `value` holds.

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
function displayName(row: Row): string
```
What a row is called on screen: `name`, else `title`, else the local id at the end of its URN. Use it wherever a row is named; never show `row.id`.
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
Formats a cell value for human-readable display; a missing or non-numeric number is `—`, never `NaN`.
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
```ts
function reportError(error: unknown): void
```
Logs an error and, in the preview frame, posts it to the Portal with file and line so the run can fix it.

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
Backend functions run in QuickJS without browser Web APIs: no `fetch`, `URLSearchParams`, `URL`, `crypto`, DOM or timers. Read data through `ctx.jc` and build strings by hand.

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

## Several endpoints (SDK-02)
An application may read up to five endpoints (e.g. `transportation` and `transportation-kpis`); the pack's THE ENDPOINTS section lists them with their types. A type served by one endpoint is read there with no extra argument. Only a type served by more than one needs `{ endpoint: "<name>" }`: in `Query` (`useEntities("Vehicle", { endpoint: "transportation" })`), in `TemporalQuery`, or as the last argument of `entities.get`/`create`/`update`/`remove`; without it the call throws a `ProblemError` naming the candidates. Writes go to the endpoint of the entity's type (an id's space picks among several). `schema()` merges every endpoint's schemas; `schema(name)` and `access(name)` read one.

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
import { useAccess, useEntities, useFilters } from "@joinedcontext/sdk";
import { BarChartCard } from "../components/charts";
import { EntityDetail } from "../components/EntityDetail";
import { EntityForm } from "../components/EntityForm";
import { EntityMap } from "../components/EntityMap";
import { EntityTable } from "../components/EntityTable";
import { FilterBar, SearchBox, SelectFilter } from "../components/filters";
import { StatTiles } from "../components/StatTiles";

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
