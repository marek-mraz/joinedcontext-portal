// @joinedcontext/sdk entry point barrel

// NGSI-LD & Schema types
export type { Cell, Column, Geo, Row } from "../ngsi";
export type { Field, FieldSchema, Schema, TypeSchema } from "../write";

// Client
export { jc, ProblemError } from "./client";
export type { Client, DataClient, Query, TemporalPoint, TemporalQuery, TemporalRow } from "./client";

// Config (read by startApp from the document the Portal serves; app code never builds a client or a
// transport, SDK-02 and SDK-06)
export type { JcConfig, JcUser } from "./config";

// Access & Permissions
export { can } from "./access";
export type { AccessDocument, Decision, Grant } from "./access";

// Design Tokens
export { applyTokens, currentTokens, DEFAULT_TOKENS, echartsTheme, mapColors, rechartsPalette, resolveTokens } from "./tokens";
export type { DesignTokens } from "./tokens";

// Hooks
export { JcProvider, useAccess, useClient, useEntities, useEntity, useFunction, useMe, useSave, useSchema } from "./hooks";
export type { Loaded } from "./hooks";

// Filters logic
export { filterRows, useFilters } from "./filters";
export type { FilterBinding, FilterDef, FilterValue } from "./filters";

// Helpers
export { aggregate, columnKind, distinct, extent, format, groupBy, pointOf, toFeatureCollection } from "./helpers";
export type { Agg, Group } from "./helpers";

// Application Bootstrap
export { startApp } from "./start";

// Components
export { Empty, ErrorBoundary, Loading, Problem, reportError } from "./components/states";
export { AppShell, navigate } from "./components/AppShell";
export type { Page } from "./components/AppShell";
export { StatTiles } from "./components/StatTiles";
export type { StatTile } from "./components/StatTiles";
export { defaultColumns, EntityTable } from "./components/EntityTable";
export type { ColumnDef } from "./components/EntityTable";
export { EntityDetail } from "./components/EntityDetail";
export { DateRangeFilter, FilterBar, RangeFilter, SearchBox, SelectFilter } from "./components/filters";
export { EntityForm, parseInput } from "./components/EntityForm";
export { ExportButton } from "./components/ExportButton";
export type { ExportFormat } from "./components/ExportButton";
export {
  BarChartCard,
  barOption,
  bucketOf,
  ChartCard,
  LineChartCard,
  lineOption,
  PieChartCard,
  pieOption,
  TimeSeriesCard,
  timeSeriesOption,
} from "./components/charts";
export type { Bucket, ChartSpec } from "./components/charts";
export { colorRamp, DECK_THRESHOLD, EntityMap, renderPath } from "./components/EntityMap";
export type { MapMode } from "./components/EntityMap";
