import React, { useCallback, useMemo, useRef, useState } from "react";
import type { RichRow, RichCell } from "./model";
import type { MetaKey, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";
import { useEntityGrid } from "./useEntityGrid";
import { opsForKind, valuesNeeded } from "./filters";
import type { ColumnFilter, FilterColumn } from "./filters";
import "./grid.css";

export interface EntityGridProps extends UseEntityGridOptions {
  renderers?: Record<string, (cell: RichCell | RichCell[] | undefined, row: RichRow) => React.ReactNode>;
  onOpenRelationship?: (urn: string) => void;
  toolbar?: React.ReactNode;
  empty?: React.ReactNode;
  className?: string;
  classNames?: Partial<Record<"root" | "table" | "header" | "row" | "cell" | "pager", string>>;
}

export function EntityGrid(props: EntityGridProps): React.JSX.Element {
  const {
    renderers,
    onOpenRelationship,
    toolbar,
    empty: emptySlot,
    className,
    classNames,
    ...hookOptions
  } = props;

  const grid = useEntityGrid(hookOptions);
  const { rows, columns, loading, error, labels, state, cellOf, toggleMeta, setOffset, setSort, setFilter, setFilterText, filterColumns, askedQuery, getGridProps, getHeaderProps, getRowProps, getCellProps } = grid;

  // A column offers a filter when it holds something `q` can ask about and the config allows it:
  // `filters.allowed` is the list a dashboard narrows its grid to.
  const allowed = hookOptions.config.filters?.allowed;
  const filterable = useMemo(() => {
    const byKey = new Map<string, FilterColumn>();
    for (const column of filterColumns) {
      if (allowed && column.attr !== null && !allowed.includes(column.attr)) {
        continue;
      }
      byKey.set(column.key, column);
    }
    return byKey;
  }, [filterColumns, allowed]);
  const asText = state.filterText !== null;

  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const totalPages = useMemo(() => {
    if (grid.total !== undefined) {
      return Math.max(1, Math.ceil(grid.total / hookOptions.config.pageSize));
    }
    // Without total, we can't know; show only next/previous
    return undefined;
  }, [grid.total, hookOptions.config.pageSize]);

  const currentPage = useMemo(() => {
    return Math.floor(state.offset / hookOptions.config.pageSize) + 1;
  }, [state.offset, hookOptions.config.pageSize]);

  const renderCellContent = useCallback((row: RichRow, column: VisibleColumn): React.ReactNode => {
    const { text, cell } = cellOf(row, column);

    // Custom renderer by attribute name first, then by cell kind
    if (column.attr && renderers?.[column.attr]) {
      return renderers[column.attr](cell, row);
    }
    if (cell && !Array.isArray(cell) && renderers?.[cell.kind]) {
      return renderers[cell.kind](cell, row);
    }

    // Relationship button
    if (column.attr && cell && !Array.isArray(cell) && cell.kind === "relationship" && onOpenRelationship) {
      const urn = typeof cell.object === "string" ? cell.object : Array.isArray(cell.object) ? cell.object.join(", ") : text;
      return (
        <button
          type="button"
          className="jc-grid-rel-btn"
          onClick={(e) => {
            e.stopPropagation();
            if (typeof cell.object === "string") onOpenRelationship(cell.object);
          }}
        >
          {urn}
        </button>
      );
    }

    return <>{text}</>;
  }, [cellOf, renderers, onOpenRelationship]);

  // Metadata menu toggle
  const toggleMenu = useCallback((attr: string) => {
    setOpenMenu((prev) => (prev === attr ? null : attr));
  }, []);

  const handleMetaCheck = useCallback((attr: string, meta: MetaKey) => {
    toggleMeta(attr, meta);
  }, [toggleMeta]);

  // Close menu on outside click
  React.useEffect(() => {
    if (!openMenu) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenu(null);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [openMenu]);

  const rootClass = `jc-grid${className ? ` ${className}` : ""}${classNames?.root ? ` ${classNames.root}` : ""}`;

  return (
    <div className={rootClass} data-density={hookOptions.config.density}>
      {toolbar && <div className="jc-grid-toolbar">{toolbar}</div>}

      <div className="jc-grid-scroll">
        <table
          className={`jc-grid-table${classNames?.table ? ` ${classNames.table}` : ""}`}
          {...getGridProps()}
        >
          <thead className={`jc-grid-thead${classNames?.header ? ` ${classNames.header}` : ""}`}>
            <tr>
              {columns.map((col, i) => (
                <th
                  key={col.key}
                  className={`jc-grid-th${col.pinned ? " jc-grid-pinned" : ""}`}
                  style={col.attr && hookOptions.config.columns?.find((c) => c.attr === col.attr)?.width ? { width: hookOptions.config.columns.find((c) => c.attr === col.attr)!.width } : undefined}
                  {...getHeaderProps(col, i)}
                >
                  <button
                    type="button"
                    className="jc-grid-sort-btn"
                    // The order is of the loaded page: the endpoint decides which rows are on it,
                    // so the label says "this page" and never promises the whole set (UI-66).
                    title={`${labels.sortPage} ${col.label}`}
                    aria-label={`${labels.sortPage} ${col.label}`}
                    onClick={() => setSort(col.attr ?? col.key)}
                  >
                    {col.label}
                    {state.sort?.attr === (col.attr ?? col.key) ? (state.sort.dir === "asc" ? " ↑" : " ↓") : ""}
                  </button>
                  {col.attr && !col.meta && (
                    <button
                      type="button"
                      className="jc-grid-meta-btn"
                      aria-label={`${labels.showMetadata} ${col.attr}`}
                      onClick={() => toggleMenu(col.attr!)}
                    >
                      ⋮
                    </button>
                  )}
                  {col.attr && !col.meta && openMenu === col.attr && (
                    <div className="jc-grid-meta-menu" ref={menuRef}>
                      {(["observedAt", "unit", "datasetId", "createdAt", "modifiedAt"] as MetaKey[]).map((meta) => {
                        const checked = (state.shown[col.attr!] ?? []).includes(meta);
                        const metaLabel = meta === "observedAt" ? labels.observedAt : meta === "unit" ? labels.unit : meta === "datasetId" ? labels.datasetId : meta === "createdAt" ? labels.createdAt : labels.modifiedAt;
                        return (
                          <label key={meta} className="jc-grid-meta-item">
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={() => handleMetaCheck(col.attr!, meta)}
                            />
                            {metaLabel}
                          </label>
                        );
                      })}
                    </div>
                  )}
                </th>
              ))}
            </tr>
            {filterable.size > 0 && (
              <tr className="jc-grid-filter-row" aria-label={labels.filterRow}>
                {columns.map((col) => {
                  const column = filterable.get(col.key);
                  return (
                    <th key={`filter-${col.key}`} className={`jc-grid-filter${col.pinned ? " jc-grid-pinned" : ""}`}>
                      {column && !asText ? (
                        <FilterCell
                          column={column}
                          label={col.label}
                          labels={labels}
                          filter={state.filters[col.key]}
                          onChange={(next) => setFilter(col.key, next)}
                        />
                      ) : null}
                    </th>
                  );
                })}
              </tr>
            )}
          </thead>
          <tbody className={`jc-grid-tbody${classNames?.row ? ` ${classNames.row}` : ""}`}>
            {rows.map((row, rowIndex) => (
              <tr key={row.id} className={`jc-grid-tr${classNames?.row ? ` ${classNames.row}` : ""}`} {...getRowProps(row, rowIndex)}>
                {columns.map((col, colIndex) => (
                  <td
                    key={`${row.id}-${col.key}`}
                    className={`jc-grid-td${col.pinned ? " jc-grid-pinned" : ""}${classNames?.cell ? ` ${classNames.cell}` : ""}`}
                    {...getCellProps(row, rowIndex, col, colIndex)}
                    title={col.pinned ? cellOf(row, col).text : undefined}
                  >
                    {renderCellContent(row, col)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {!loading && rows.length === 0 && !error && (
        <div className="jc-grid-empty">{emptySlot ?? labels.empty}</div>
      )}

      {loading && <div className="jc-grid-loading">{labels.loading}</div>}

      {error && <div className="jc-grid-error">{labels.error}: {error}</div>}

      {filterable.size > 0 && (
        <div className="jc-grid-query">
          <label className="jc-grid-query-text">
            <span>{labels.query}</span>
            {asText ? (
              // The person owns the query from here: the row cannot show every `q` NGSI-LD allows
              // (a `|`, a bracket), so taking it over is how those are written at all.
              <input
                className="jc-grid-query-input"
                value={state.filterText ?? ""}
                onChange={(e) => setFilterText(e.target.value)}
              />
            ) : (
              <output className="jc-grid-query-value">{queryText(askedQuery)}</output>
            )}
          </label>
          <button
            type="button"
            onClick={() => {
              void navigator.clipboard?.writeText(queryText(askedQuery));
            }}
          >
            {labels.copyQuery}
          </button>
          <label className="jc-grid-query-switch">
            <input
              type="checkbox"
              checked={asText}
              onChange={(e) => setFilterText(e.target.checked ? (askedQuery.q ?? "") : null)}
            />
            {labels.editAsText}
          </label>
        </div>
      )}

      <div className={`jc-grid-pager${classNames?.pager ? ` ${classNames.pager}` : ""}`}>
        <button
          type="button"
          disabled={state.offset === 0}
          onClick={() => setOffset(Math.max(0, state.offset - hookOptions.config.pageSize))}
        >
          {labels.previous}
        </button>
        <span className="jc-grid-page-info">{`${labels.page} ${currentPage}`}</span>
        {totalPages !== undefined && <span className="jc-grid-page-count">{`/ ${totalPages}`}</span>}
        {/* The endpoint's own count, not the page's: an answer the gateway narrowed carries none,
            and then the footer says nothing rather than a number it guessed (R22). */}
        {grid.total !== undefined && (
          <span className="jc-grid-total">{`${grid.total} ${labels.matching}`}</span>
        )}
        <button
          type="button"
          disabled={totalPages !== undefined ? currentPage >= totalPages : rows.length < hookOptions.config.pageSize}
          onClick={() => setOffset(state.offset + hookOptions.config.pageSize)}
        >
          {labels.next}
        </button>
      </div>
    </div>
  );
}

/** The query as a person reads and copies it: the `q`, and the id pattern when one is asked. */
function queryText(asked: { q?: string; idPattern?: string }): string {
  const parts: string[] = [];
  if (asked.q) {
    parts.push(`q=${asked.q}`);
  }
  if (asked.idPattern) {
    parts.push(`idPattern=${asked.idPattern}`);
  }
  return parts.join("&");
}

/**
 * One column's filter: the operators its content allows, and the value(s) the chosen one needs.
 * A filter is sent only once it is complete, so nothing narrows while it is being typed.
 */
function FilterCell({
  column,
  label,
  labels,
  filter,
  onChange,
}: {
  column: FilterColumn;
  label: string;
  labels: { filter: string; value: string; upperValue: string; ops: Record<string, string> };
  filter: ColumnFilter | undefined;
  onChange: (next: ColumnFilter | undefined) => void;
}): React.JSX.Element {
  const ops = opsForKind(column.kind);
  const op = filter?.op ?? ops[0];
  const needed = valuesNeeded(op);
  const type = column.kind === "number" ? "number" : column.kind === "date" ? "date" : "text";
  return (
    <div className="jc-grid-filter-cell">
      <select
        aria-label={`${labels.filter}: ${label}`}
        value={filter ? op : ""}
        onChange={(e) => {
          const next = e.target.value as ColumnFilter["op"] | "";
          if (next === "") {
            onChange(undefined);
            return;
          }
          onChange({ op: next, value: filter?.value ?? "", value2: filter?.value2 });
        }}
      >
        <option value="">—</option>
        {ops.map((each) => (
          <option key={each} value={each}>
            {labels.ops[each] ?? each}
          </option>
        ))}
      </select>
      {filter && needed >= 1 && (
        <input
          aria-label={`${labels.value}: ${label}`}
          type={type}
          value={filter.value}
          onChange={(e) => onChange({ ...filter, value: e.target.value })}
        />
      )}
      {filter && needed === 2 && (
        <input
          aria-label={`${labels.upperValue}: ${label}`}
          type={type}
          value={filter.value2 ?? ""}
          onChange={(e) => onChange({ ...filter, value2: e.target.value })}
        />
      )}
    </div>
  );
}
