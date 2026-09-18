import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ResolvedGridConfig, GridColumn } from "./config";
import type { EntitySource, GridQuery } from "./source";
import type { RichRow, RichCell } from "./model";
import { attributesOf, cellText } from "./model";

export type MetaKey = "observedAt" | "unit" | "datasetId" | "createdAt" | "modifiedAt";

export interface GridState {
  offset: number;
  activeCell: { row: number; col: number } | null;
  selected: string[];
  shown: Record<string, MetaKey[]>;
  sort: { attr: string; dir: "asc" | "desc" } | null;
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
  const merged = { ...base };
  for (const key of Object.keys(partial) as (keyof GridLabels)[]) {
    if (partial[key] !== undefined) merged[key] = partial[key]!;
  }
  return merged;
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

export function useEntityGrid(options: UseEntityGridOptions): EntityGrid {
  const { config, source, labels: labelsPartial, state: controlledState, onStateChange, query: queryPartial } = options;
  const labels = useMemo(() => mergeLabels(DEFAULT_LABELS, labelsPartial), [labelsPartial]);

  // Internal state (uncontrolled)
  const [internalOffset, setInternalOffset] = useState(0);
  const [internalActiveCell, setInternalActiveCell] = useState<{ row: number; col: number } | null>(null);
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

  // Controlled state: use controlled values when key present
  const offset = controlledState?.offset !== undefined ? controlledState.offset : internalOffset;
  const activeCell = controlledState?.activeCell !== undefined ? controlledState.activeCell : internalActiveCell;
  const selected = controlledState?.selected !== undefined ? controlledState.selected : internalSelected;
  const shown = controlledState?.shown !== undefined ? controlledState.shown : internalShown;
  const sort = controlledState?.sort !== undefined ? controlledState.sort : internalSort;

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

  const fetchData = useCallback(() => {
    const nonce = ++nonceRef.current;
    cancelledRef.current = false;
    setLoading(true);
    setError(null);

    const gridQuery: GridQuery = {
      type: config.type,
      q: queryPartial?.q ?? config.filters.preset?.q,
      attrs: queryPartial?.attrs ?? config.filters.preset?.attrs,
      idPattern: queryPartial?.idPattern ?? config.filters.preset?.idPattern,
      scopeQ: queryPartial?.scopeQ ?? config.filters.preset?.scopeQ,
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
  }, [source, config.type, config.pageSize, config.filters.preset, offset, queryPartial]);

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
        onStateChange({ offset, activeCell, selected, shown: next, sort });
      }
      if (controlledState?.shown === undefined) {
        setInternalShown(next);
      }
    },
    [shown, offset, activeCell, selected, sort, onStateChange, controlledState],
  );

  const setOffset = useCallback(
    (newOffset: number) => {
      if (onStateChange) {
        onStateChange({ offset: newOffset, activeCell, selected, shown, sort });
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(newOffset);
      }
    },
    [offset, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  const setSort = useCallback(
    (attr: string) => {
      const next = sort?.attr === attr && sort.dir === "asc" ? { attr, dir: "desc" as const } : { attr, dir: "asc" as const };
      if (onStateChange) {
        onStateChange({ offset, activeCell, selected, shown, sort: next });
      }
      if (controlledState?.sort === undefined) {
        setInternalSort(next);
      }
    },
    [sort, offset, activeCell, selected, shown, onStateChange, controlledState],
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
        onStateChange({ offset, activeCell: next, selected, shown, sort });
      }
      if (controlledState?.activeCell === undefined) {
        setInternalActiveCell(next);
      }
      return true;
    },
    [activeCell, sortedRows.length, columns.length, offset, selected, shown, sort, onStateChange, controlledState],
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
    state: { offset, activeCell, selected, shown, sort },
    labels,
    cellOf,
    toggleMeta,
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
