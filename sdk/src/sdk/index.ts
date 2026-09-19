// @joinedcontext/sdk entry point barrel

// NGSI-LD & Schema types
export type { Cell, Column, Geo, Row } from "../ngsi";
export { fieldOf } from "../write";
export type { Field, FieldSchema, Schema, TypeSchema } from "../write";

// Client
export { jc, ProblemError } from "./client";
export type { Client, DataClient, EndpointOption, Query, TemporalPoint, TemporalQuery, TemporalRow } from "./client";

// Config (read by startApp from the document the Portal serves; app code never builds a client or a
// transport, SDK-02 and SDK-06)
export type { JcConfig, JcEndpoint, JcUser } from "./config";

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
export { aggregate, columnKind, displayName, distinct, extent, format, groupBy, pointOf, toFeatureCollection } from "./helpers";
export type { Agg, Group } from "./helpers";

// The entity grid (UI-64, UI-71, SDK-29): one component for the Portal, Dashboards and apps
export { EntityGrid } from "../grid/EntityGrid";
export type { EntityGridProps } from "../grid/EntityGrid";
export { DEFAULT_LABELS, useEntityGrid } from "../grid/useEntityGrid";
export type { GridLabels, GridState, MetaKey, UseEntityGridOptions, VisibleColumn } from "../grid/useEntityGrid";
export { DEFAULT_PAGE_SIZE, gridConfigSchema, MAX_PAGE_SIZE, parseGridConfig } from "../grid/config";
export type { ConfigFinding, EntityGridConfig, GridColumn, GridSource, ResolvedGridConfig } from "../grid/config";
export { endpointSource, fixtureSource, historyOf, SourceError, sourceFor, spaceSource } from "../grid/source";
export type { EntitySource, GridPage, GridQuery, HistoryPoint, HistoryWindow } from "../grid/source";
export { attributesOf, cellText, projectRow, toRichCell, toRichRow } from "../grid/model";
export type { CellKind, RichCell, RichRow } from "../grid/model";
export { andQ, opsForKind, queryFromFilters } from "../grid/filters";
export type { ColumnFilter, FilterKind, FilterOp } from "../grid/filters";
export { applyChanges, attrsBody, MAX_ENTITIES } from "../grid/apply";
export type { ApplyResult, AttributeChange, EntityChange, Observed } from "../grid/apply";
export { asCsv, EntityHistory, MAX_POINTS } from "../grid/EntityHistory";
export type { HistoryLabels } from "../grid/EntityHistory";
// The host's own transport, for a page that renders the grid itself (T-1439).
export { originTransport, transportFor } from "./transport";

// Tables, maps, exports: what the template's components stand on
export { compare } from "../views/Table";
export { mapWorkerReady, NO_BASEMAP, NO_LOCATIONS, styleFor } from "./map";
export { download, toCsv, toGeoJson, toPdf, toPng } from "../artifact";

// Application Bootstrap
export { startApp } from "./start";
export { reportError } from "./report";
