import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ResolvedGridConfig, GridColumn } from "./config";
import type { GeoArea } from "./geoarea";
import { areaQuery } from "./geoarea";
import type { EntitySource, GridQuery } from "./source";
import type { RichRow, RichCell } from "./model";
import { attributesOf, cellText } from "./model";
import { andQ, opsForKind, queryFromFilters } from "./filters";
import type { ColumnFilter, FilterColumn, FilterKind, FilterOp } from "./filters";
import type { AttributeChange, EntityChange } from "./apply";

export type MetaKey = "observedAt" | "unit" | "datasetId" | "createdAt" | "modifiedAt";

export interface GridState {
  offset: number;
  activeCell: { row: number; col: number } | null;
  selected: string[];
  shown: Record<string, MetaKey[]>;
  sort: { attr: string; dir: "asc" | "desc" } | null;
  /** One filter per column key, as the filter row holds them (UI-66). */
  filters: Record<string, ColumnFilter>;
  /** The `q` typed by hand instead of the row; `null` while the row is what asks. */
  filterText: string | null;
  /** Cells a person changed and has not applied: entity id, then attribute (UI-67). */
  edits: Record<string, Record<string, unknown>>;
}

export interface GridLabels {
  id: string;
  type: string;
  createdAt: string;
  modifiedAt: string;
  observedAt: string;
  unit: string;
  datasetId: string;
  empty: string;
  loading: string;
  previous: string;
  next: string;
  page: string;
  showMetadata: string;
  error: string;
  /** The filter row: the operators, by name, and what the query under the grid is called. */
  filter: string;
  ops: Record<FilterOp, string>;
  value: string;
  upperValue: string;
  query: string;
  copyQuery: string;
  editAsText: string;
  filterRow: string;
  /** Said on a sortable header, because the order is of the loaded page only (UI-66). */
  sortPage: string;
  /** What the footer calls the endpoint's own count of matching entities. */
  matching: string;
  /** The entry that opens one attribute's history, and the panel's own labels (T-1431). */
  history: string;
  historyLabels?: Partial<import("./EntityHistory").HistoryLabels>;
  /** Edit mode: the cell editor, the pending list and what applying them is called (UI-67). */
  edit: string;
  pending: string;
  review: string;
  apply: string;
  discard: string;
  observedKeep: string;
  observedNow: string;
  applying: string;
  refusedHere: string;
}

export const DEFAULT_LABELS: GridLabels = {
  id: "ID",
  type: "Type",
  createdAt: "Created",
  modifiedAt: "Modified",
  observedAt: "Observed",
  unit: "Unit",
  datasetId: "Dataset",
  empty: "No rows.",
  loading: "Loading…",
  previous: "Previous",
  next: "Next",
  page: "Page",
  showMetadata: "Show metadata for",
  error: "Error",
  filter: "Filter",
  ops: {
    contains: "contains",
    equals: "is",
    notEquals: "is not",
    gt: ">",
    gte: "≥",
    lt: "<",
    lte: "≤",
    between: "between",
    empty: "is empty",
    present: "has a value",
    pattern: "matches",
  },
  value: "Value",
  upperValue: "Upper value",
  query: "The query this asks",
  copyQuery: "Copy the query",
  editAsText: "Edit as text",
  filterRow: "Filters",
  sortPage: "Sort this page by",
  matching: "matching",
  history: "History",
  edit: "Edit",
  pending: "not applied yet",
  review: "Review the changes",
  apply: "Apply",
  discard: "Discard the changes",
  observedKeep: "keep when each value was observed",
  observedNow: "these values were observed now",
  applying: "Applying…",
  refusedHere: "refused",
};

export interface VisibleColumn {
  key: string;
  attr: string | null;
  meta: MetaKey | null;
  label: string;
  pinned: boolean;
}

export interface UseEntityGridOptions {
  config: ResolvedGridConfig;
  source: EntitySource;
  labels?: Partial<GridLabels>;
  state?: Partial<GridState>;
  onStateChange?: (next: GridState) => void;
  query?: Omit<GridQuery, "type">;
}

export interface EntityGrid {
  rows: RichRow[];
  columns: VisibleColumn[];
  total?: number;
  loading: boolean;
  error: string | null;
  state: GridState;
  labels: GridLabels;
  cellOf(row: RichRow, column: VisibleColumn): { text: string; cell?: RichCell | RichCell[] };
  toggleMeta(attr: string, meta: MetaKey): void;
  /** The filter of one column, or `undefined` to drop it; the answer starts again at page one. */
  setFilter(key: string, filter: ColumnFilter | undefined): void;
  /** Hands the query to the person as text, or gives the row back (`null`). */
  setFilterText(text: string | null): void;
  /** Every column the filter row can ask about, with what it holds. */
  filterColumns: FilterColumn[];
  /** One cell a person changed; `undefined` gives the endpoint's own value back. */
  setEdit(id: string, attr: string, value: unknown | undefined): void;
  /** Forgets every edit that has not been applied. */
  clearEdits(): void;
  /** What applying would send: one entry per entity, with what each cell held before. */
  pendingChanges: EntityChange[];
  /** The query the filters ask for right now, as the endpoint receives it. */
  askedQuery: { q?: string; idPattern?: string };
  /** The area drawn on the map, which the filter row cannot hold (UI-72); `null` for none. */
  area: GeoArea | null;
  /** Puts the active cell on a row and column outright, for a selection from the map (UI-72). */
  setActive(row: number, col: number): void;
  /** Draws the area or removes it; either way the answer starts again at page one. */
  setArea(area: GeoArea | null): void;
  setOffset(offset: number): void;
  setSort(attr: string): void;
  moveActive(key: string): boolean;
  getGridProps(): Record<string, unknown>;
  getHeaderProps(column: VisibleColumn, index: number): Record<string, unknown>;
  getRowProps(row: RichRow, index: number): Record<string, unknown>;
  getCellProps(row: RichRow, rowIndex: number, column: VisibleColumn, colIndex: number): Record<string, unknown>;
  reload(): void;
}

function mergeLabels(base: GridLabels, partial?: Partial<GridLabels>): GridLabels {
  if (!partial) return base;
  // The operator names are a group of their own: a caller that translates two of them keeps the
  // rest, instead of leaving the row with holes where a label used to be.
  return { ...base, ...partial, ops: { ...base.ops, ...partial.ops } };
}

const META_LABEL: Record<MetaKey, keyof GridLabels> = {
  observedAt: "observedAt",
  unit: "unit",
  datasetId: "datasetId",
  createdAt: "createdAt",
  modifiedAt: "modifiedAt",
};

function buildColumns(
  config: ResolvedGridConfig,
  rows: RichRow[],
  shown: Record<string, MetaKey[]>,
  labels: GridLabels,
): VisibleColumn[] {
  const cols: VisibleColumn[] = [];

  // id column (pinned)
  cols.push({ key: "id", attr: null, meta: null, label: labels.id, pinned: true });

  // entity timestamps
  if (config.entityTimestamps) {
    cols.push({ key: "entity:createdAt", attr: null, meta: null, label: labels.createdAt, pinned: false });
    cols.push({ key: "entity:modifiedAt", attr: null, meta: null, label: labels.modifiedAt, pinned: false });
  }

  const attrList = config.columns.length > 0 ? config.columns.map((c) => c.attr) : attributesOf(rows);
  const colMap = new Map<string, GridColumn>();
  for (const c of config.columns) {
    colMap.set(c.attr, c);
  }

  for (const attr of attrList) {
    const gridCol = colMap.get(attr);
    const label = gridCol?.label ?? attr;
    const pinned = gridCol?.pinned ?? false;
    cols.push({ key: attr, attr, meta: null, label, pinned });

    const metaKeys = shown[attr] ?? [];
    for (const meta of metaKeys) {
      cols.push({ key: `${attr}:${meta}`, attr, meta, label: `${label} · ${labels[META_LABEL[meta]]}`, pinned: false });
    }
  }

  return cols;
}

function metaOf(cell: RichCell, meta: MetaKey): string | undefined {
  return meta === "unit" ? cell.unitCode : cell[meta];
}

function cellTextWithUnit(cell: RichCell | RichCell[] | undefined, column: VisibleColumn): string {
  const text = cellText(cell);
  if (column.meta) return text;
  if (!Array.isArray(cell) && cell && cell.kind === "property" && cell.unitCode && !column.meta) {
    return text + " " + cell.unitCode;
  }
  return text;
}

const META_KIND: Record<MetaKey, FilterKind> = {
  observedAt: "date",
  createdAt: "date",
  modifiedAt: "date",
  unit: "text",
  datasetId: "text",
};

const DATE_VALUE = /^\d{4}-\d{2}-\d{2}([T ]|$)/;

/**
 * What a column holds, as far as a filter cares: the configured format when the config names one,
 * else what the loaded rows show. Without a row and without a format a column is text, which is
 * the only guess that cannot refuse a value the endpoint would have accepted.
 */
export function filterKindOf(
  column: VisibleColumn,
  config: ResolvedGridConfig,
  rows: RichRow[],
): FilterKind {
  if (column.key === "id") {
    return "id";
  }
  if (column.attr === null) {
    // The entity's own timestamps are not part of `q`; NGSI-LD filters them with its own
    // temporal parameters, which the grid does not send.
    return "none";
  }
  if (column.meta) {
    return META_KIND[column.meta];
  }
  const format = config.columns.find((c) => c.attr === column.attr)?.format;
  if (format === "number") {
    return "number";
  }
  if (format === "date") {
    return "date";
  }
  if (format === "text" || format === "link") {
    return "text";
  }
  for (const row of rows) {
    const cell = row.cells[column.attr];
    const one = Array.isArray(cell) ? cell[0] : cell;
    if (!one) {
      continue;
    }
    if (one.kind === "relationship") {
      return "relationship";
    }
    if (one.kind === "geo") {
      return "geo";
    }
    if (typeof one.value === "number") {
      return "number";
    }
    if (typeof one.value === "string" && DATE_VALUE.test(one.value)) {
      return "date";
    }
    return "text";
  }
  return "text";
}

export function useEntityGrid(options: UseEntityGridOptions): EntityGrid {
  const { config, source, labels: labelsPartial, state: controlledState, onStateChange, query: queryPartial } = options;
  const labels = useMemo(() => mergeLabels(DEFAULT_LABELS, labelsPartial), [labelsPartial]);

  // Internal state (uncontrolled)
  const [internalOffset, setInternalOffset] = useState(0);
  const [internalActiveCell, setInternalActiveCell] = useState<{ row: number; col: number } | null>(null);
  // The drawn area is the map's own filter and lives beside the row's: a `GridState` a host stores
  // holds the typed filters, and an area is a shape a person drew in this session.
  const [area, setAreaState] = useState<GeoArea | null>(null);
  const [internalSelected] = useState<string[]>([]);
  const [internalShown, setInternalShown] = useState<Record<string, MetaKey[]>>(() => {
    const init: Record<string, MetaKey[]> = {};
    for (const col of config.columns) {
      if (col.show) {
        const keys: MetaKey[] = [];
        if (col.show.observedAt) keys.push("observedAt");
        if (col.show.unit) keys.push("unit");
        if (col.show.datasetId) keys.push("datasetId");
        if (col.show.createdAt) keys.push("createdAt");
        if (col.show.modifiedAt) keys.push("modifiedAt");
        if (keys.length > 0) init[col.attr] = keys;
      }
    }
    return init;
  });
  const [internalSort, setInternalSort] = useState<{ attr: string; dir: "asc" | "desc" } | null>(null);
  const [internalFilters, setInternalFilters] = useState<Record<string, ColumnFilter>>({});
  const [internalFilterText, setInternalFilterText] = useState<string | null>(null);
  const [internalEdits, setInternalEdits] = useState<Record<string, Record<string, unknown>>>({});

  // Controlled state: use controlled values when key present
  const offset = controlledState?.offset !== undefined ? controlledState.offset : internalOffset;
  const activeCell = controlledState?.activeCell !== undefined ? controlledState.activeCell : internalActiveCell;
  const selected = controlledState?.selected !== undefined ? controlledState.selected : internalSelected;
  const shown = controlledState?.shown !== undefined ? controlledState.shown : internalShown;
  const sort = controlledState?.sort !== undefined ? controlledState.sort : internalSort;
  const filters = controlledState?.filters !== undefined ? controlledState.filters : internalFilters;
  const filterText =
    controlledState?.filterText !== undefined ? controlledState.filterText : internalFilterText;
  const edits = controlledState?.edits !== undefined ? controlledState.edits : internalEdits;

  const [rows, setRows] = useState<RichRow[]>([]);
  const [total, setTotal] = useState<number | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const cancelledRef = useRef(false);
  const nonceRef = useRef(0);

  const columns = useMemo(() => buildColumns(config, rows, shown, labels), [config, rows, shown, labels]);
  // Sorting orders the loaded page only; the server's order decides which rows are on it (UI-66).
  const sortedRows = useMemo(() => {
    if (!sort) return rows;
    const factor = sort.dir === "asc" ? 1 : -1;
    const key = (row: RichRow) => cellText(row.cells[sort.attr]);
    return [...rows].sort((a, b) => {
      const x = key(a);
      const y = key(b);
      const nx = Number(x);
      const ny = Number(y);
      const byNumber = x !== "" && y !== "" && !Number.isNaN(nx) && !Number.isNaN(ny);
      return factor * (byNumber ? nx - ny : x.localeCompare(y));
    });
  }, [rows, sort]);

  const filterColumns = useMemo(
    (): FilterColumn[] =>
      columns
        .map((column) => ({
          key: column.key,
          attr: column.attr,
          meta: column.meta,
          kind: filterKindOf(column, config, rows),
        }))
        .filter((column) => opsForKind(column.kind).length > 0),
    [columns, config, rows],
  );

  // What the endpoint is asked for: the typed query when the person took it over, else the row's.
  const askedQuery = useMemo(() => {
    const built = queryFromFilters(filterColumns, filters);
    return filterText === null ? built : { q: filterText.trim() || undefined, idPattern: built.idPattern };
  }, [filterColumns, filters, filterText]);

  /**
   * What applying the pending cells would send: the value before, the unit it was measured in and
   * whether the attribute is a Relationship all come from the row the person edited, so the
   * review reads what they saw and the write keeps what it must not lose.
   */
  const pendingChanges = useMemo((): EntityChange[] => {
    const byId = new Map(rows.map((row) => [row.id, row]));
    return Object.entries(edits).map(([id, attrs]) => {
      const row = byId.get(id);
      const changes: AttributeChange[] = Object.entries(attrs).map(([attribute, after]) => {
        const cell = row?.cells[attribute];
        const one = Array.isArray(cell) ? cell[0] : cell;
        return {
          attribute,
          after,
          before: one?.kind === "relationship" ? one.object : one?.value,
          unitCode: one?.unitCode,
          // The cell's own kind decides how it is written: a geometry as a GeoProperty, so the
          // attribute keeps the type the model gave it (UI-72).
          kind: one?.kind === "relationship" ? "relationship" : one?.kind === "geo" ? "geo" : "property",
        };
      });
      return { id, changes };
    });
  }, [edits, rows]);

  const fetchData = useCallback(() => {
    const nonce = ++nonceRef.current;
    cancelledRef.current = false;
    setLoading(true);
    setError(null);

    const gridQuery: GridQuery = {
      type: config.type,
      // The caller's query and the filter row are both true: `;` is how NGSI-LD says "and", so a
      // preset that narrows the grid to one district cannot be widened by a filter (EP-07).
      q: andQ(queryPartial?.q ?? config.filters.preset?.q, askedQuery.q),
      attrs: queryPartial?.attrs ?? config.filters.preset?.attrs,
      idPattern: askedQuery.idPattern ?? queryPartial?.idPattern ?? config.filters.preset?.idPattern,
      scopeQ: queryPartial?.scopeQ ?? config.filters.preset?.scopeQ,
      // `areaQuery` refuses a shape that is not an area, so a half-drawn one asks for nothing
      // rather than for everything.
      area: areaQuery(area) ?? undefined,
    };

    source
      .query(gridQuery, { offset, limit: config.pageSize })
      .then((page) => {
        if (cancelledRef.current || nonceRef.current !== nonce) return;
        setRows(page.rows);
        setTotal(page.total);
        setLoading(false);
      })
      .catch((err) => {
        if (cancelledRef.current || nonceRef.current !== nonce) return;
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });
  }, [source, config.type, config.pageSize, config.filters.preset, offset, queryPartial, askedQuery, area]);

  useEffect(() => {
    fetchData();
    return () => {
      cancelledRef.current = true;
    };
  }, [fetchData]);

  const reload = useCallback(() => {
    fetchData();
  }, [fetchData]);

  const cellOf = useCallback(
    (row: RichRow, column: VisibleColumn): { text: string; cell?: RichCell | RichCell[] } => {
      if (column.attr === null) {
        // id or entity timestamp
        if (column.key === "id") return { text: row.id };
        if (column.key === "entity:createdAt") return { text: row.createdAt ?? "" };
        if (column.key === "entity:modifiedAt") return { text: row.modifiedAt ?? "" };
        return { text: "" };
      }
      const cell = row.cells[column.attr];
      if (column.meta) {
        // metadata sub-column
        if (Array.isArray(cell)) {
          const texts = cell.map((c) => {
            const val = metaOf(c, column.meta!);
            return val == null ? "" : String(val);
          });
          return { text: texts.join(" | "), cell };
        }
        if (cell) {
          const val = metaOf(cell, column.meta);
          return { text: val == null ? "" : String(val), cell };
        }
        return { text: "" };
      }
      return { text: cellTextWithUnit(cell, column), cell };
    },
    [],
  );

  const toggleMeta = useCallback(
    (attr: string, meta: MetaKey) => {
      const next = { ...shown };
      const current = next[attr] ?? [];
      if (current.includes(meta)) {
        next[attr] = current.filter((m) => m !== meta);
        if (next[attr].length === 0) delete next[attr];
      } else {
        next[attr] = [...current, meta];
      }
      if (onStateChange) {
        onStateChange({ offset, activeCell, selected, shown: next, sort, filters, filterText, edits });
      }
      if (controlledState?.shown === undefined) {
        setInternalShown(next);
      }
    },
    [shown, offset, activeCell, selected, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  const setOffset = useCallback(
    (newOffset: number) => {
      if (onStateChange) {
        onStateChange({ offset: newOffset, activeCell, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(newOffset);
      }
    },
    [activeCell, selected, shown, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  const setSort = useCallback(
    (attr: string) => {
      const next = sort?.attr === attr && sort.dir === "asc" ? { attr, dir: "desc" as const } : { attr, dir: "asc" as const };
      if (onStateChange) {
        onStateChange({ offset, activeCell, selected, shown, sort: next, filters, filterText, edits });
      }
      if (controlledState?.sort === undefined) {
        setInternalSort(next);
      }
    },
    [sort, offset, activeCell, selected, shown, onStateChange, controlledState],
  );

  /**
   * One column's filter. The answer starts again at the first page: a filter applied on page
   * three would otherwise show an empty page of a set that does have matches.
   */
  const setFilter = useCallback(
    (key: string, filter: ColumnFilter | undefined) => {
      const next = { ...filters };
      if (filter === undefined) {
        delete next[key];
      } else {
        next[key] = filter;
      }
      if (onStateChange) {
        onStateChange({ offset: 0, activeCell, selected, shown, sort, filters: next, filterText, edits });
      }
      if (controlledState?.filters === undefined) {
        setInternalFilters(next);
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(0);
      }
    },
    [filters, filterText, edits, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  /**
   * Moves the active cell outright, for a selection that came from somewhere other than the keyboard
   * or a cell click — the map's shapes (UI-72). `moveActive` walks by one key; this puts it where a
   * caller says, clamped to the page so a stale row index cannot point off it.
   */
  const setActive = useCallback(
    (row: number, col: number) => {
      const next = {
        row: Math.max(0, Math.min(row, Math.max(0, sortedRows.length - 1))),
        col: Math.max(0, Math.min(col, Math.max(0, columns.length - 1))),
      };
      if (onStateChange) {
        onStateChange({ offset, activeCell: next, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.activeCell === undefined) {
        setInternalActiveCell(next);
      }
    },
    [sortedRows.length, columns.length, offset, selected, shown, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  /**
   * The drawn area, or `null` to remove it. Like a filter, a new area starts the answer at page one:
   * the page the person was on is a page of a different question.
   */
  const setArea = useCallback(
    (next: GeoArea | null) => {
      setAreaState(next);
      if (onStateChange) {
        onStateChange({ offset: 0, activeCell, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(0);
      }
    },
    [filters, filterText, edits, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  /**
   * One cell a person changed. A value equal to what the endpoint answered is not a change, so
   * typing a value back removes it from the pending list rather than sending it again.
   */
  const setEdit = useCallback(
    (id: string, attr: string, value: unknown | undefined) => {
      const next: Record<string, Record<string, unknown>> = { ...edits };
      const forEntity = { ...(next[id] ?? {}) };
      if (value === undefined) {
        delete forEntity[attr];
      } else {
        forEntity[attr] = value;
      }
      if (Object.keys(forEntity).length === 0) {
        delete next[id];
      } else {
        next[id] = forEntity;
      }
      if (onStateChange) {
        onStateChange({ offset, activeCell, selected, shown, sort, filters, filterText, edits: next });
      }
      if (controlledState?.edits === undefined) {
        setInternalEdits(next);
      }
    },
    [edits, offset, activeCell, selected, shown, sort, filters, filterText, onStateChange, controlledState],
  );

  const clearEdits = useCallback(() => {
    if (onStateChange) {
      onStateChange({ offset, activeCell, selected, shown, sort, filters, filterText, edits: {} });
    }
    if (controlledState?.edits === undefined) {
      setInternalEdits({});
    }
  }, [offset, activeCell, selected, shown, sort, filters, filterText, onStateChange, controlledState]);

  /** The query as text: the rows compose the first version, and the person owns it from then on. */
  const setFilterText = useCallback(
    (text: string | null) => {
      if (onStateChange) {
        onStateChange({ offset: 0, activeCell, selected, shown, sort, filters, filterText: text, edits });
      }
      if (controlledState?.filterText === undefined) {
        setInternalFilterText(text);
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(0);
      }
    },
    [filters, edits, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  const moveActive = useCallback(
    (key: string): boolean => {
      const maxRow = sortedRows.length - 1;
      const maxCol = columns.length - 1;
      let { row, col } = activeCell ?? { row: 0, col: 0 };

      switch (key) {
        case "ArrowUp":
          if (row > 0) row--;
          else return false;
          break;
        case "ArrowDown":
          if (row < maxRow) row++;
          else return false;
          break;
        case "ArrowLeft":
          if (col > 0) col--;
          else return false;
          break;
        case "ArrowRight":
          if (col < maxCol) col++;
          else return false;
          break;
        case "Home":
          col = 0;
          break;
        case "End":
          col = maxCol;
          break;
        case "PageUp":
          row = Math.max(0, row - Math.max(1, sortedRows.length));
          break;
        case "PageDown":
          row = Math.min(maxRow, row + Math.max(1, sortedRows.length));
          break;
        default:
          return false;
      }

      const next = { row, col };
      if (onStateChange) {
        onStateChange({ offset, activeCell: next, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.activeCell === undefined) {
        setInternalActiveCell(next);
      }
      return true;
    },
    [activeCell, sortedRows.length, columns.length, offset, selected, shown, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  const getGridProps = useCallback((): Record<string, unknown> => {
    return {
      role: "grid",
      "aria-rowcount": rows.length + 1,
      "aria-colcount": columns.length,
      tabIndex: 0,
      onKeyDown: (e: React.KeyboardEvent) => {
        if (moveActive(e.key)) {
          e.preventDefault();
        }
      },
    };
  }, [rows.length, columns.length, moveActive]);

  const getHeaderProps = useCallback(
    (_column: VisibleColumn, index: number): Record<string, unknown> => {
      return {
        role: "columnheader",
        "aria-colindex": index + 1,
      };
    },
    [],
  );

  const getRowProps = useCallback(
    (_row: RichRow, index: number): Record<string, unknown> => {
      return {
        role: "row",
        "aria-rowindex": index + 2,
      };
    },
    [],
  );

  const getCellProps = useCallback(
    (_row: RichRow, rowIndex: number, _column: VisibleColumn, colIndex: number): Record<string, unknown> => {
      const isActive = activeCell?.row === rowIndex && activeCell?.col === colIndex;
      return {
        role: "gridcell",
        "aria-colindex": colIndex + 1,
        "aria-selected": isActive,
        "data-active": isActive ? "" : undefined,
      };
    },
    [activeCell],
  );

  return {
    rows: sortedRows,
    columns,
    total,
    loading,
    error,
    state: { offset, activeCell, selected, shown, sort, filters, filterText, edits },
    labels,
    cellOf,
    toggleMeta,
    setFilter,
    setFilterText,
    filterColumns,
    askedQuery,
    area,
    setArea,
    setActive,
    setEdit,
    clearEdits,
    pendingChanges,
    setOffset,
    setSort,
    moveActive,
    getGridProps,
    getHeaderProps,
    getRowProps,
    getCellProps,
    reload,
  };
}
