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

// Tables, maps, exports: what the template's components stand on
export { compare } from "../views/Table";
export { mapWorkerReady, NO_BASEMAP, NO_LOCATIONS, styleFor } from "./map";
export { download, toCsv, toGeoJson, toPdf, toPng } from "../artifact";

// Application Bootstrap
export { startApp } from "./start";
export { reportError } from "./report";
