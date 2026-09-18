import React, { useCallback, useMemo, useRef, useState } from "react";
import type { RichRow, RichCell } from "./model";
import type { MetaKey, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";
import { useEntityGrid } from "./useEntityGrid";
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
  const { rows, columns, loading, error, labels, state, cellOf, toggleMeta, setOffset, getGridProps, getHeaderProps, getRowProps, getCellProps } = grid;

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
                  <span>{col.label}</span>
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
