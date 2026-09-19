import React, { useCallback, useMemo, useRef, useState } from "react";
import type { RichRow, RichCell } from "./model";
import type { MetaKey, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";
import { useEntityGrid } from "./useEntityGrid";
import { opsForKind, valuesNeeded } from "./filters";
import type { ColumnFilter, FilterColumn } from "./filters";
import { applyChanges, MAX_ENTITIES } from "./apply";
import type { Observed, Refusal } from "./apply";
import { EntityHistory } from "./EntityHistory";
import "./grid.css";

export interface EntityGridProps extends UseEntityGridOptions {
  /**
   * How one column's cells are drawn, by attribute name, by cell kind, or `id` for the pinned
   * identifier column — which is how a host makes a row open something of its own (a detail pane,
   * a row action) without a second table of its own.
   */
  renderers?: Record<string, (cell: RichCell | RichCell[] | undefined, row: RichRow) => React.ReactNode>;
  onOpenRelationship?: (urn: string) => void;
  /**
   * The page of rows as the source answered it, with the offset it starts at, for a host that
   * exports or counts what is shown.
   */
  onRows?: (rows: RichRow[], offset: number) => void;
  toolbar?: React.ReactNode;
  empty?: React.ReactNode;
  className?: string;
  classNames?: Partial<Record<"root" | "table" | "header" | "row" | "cell" | "pager", string>>;
}

export function EntityGrid(props: EntityGridProps): React.JSX.Element {
  const {
    renderers,
    onOpenRelationship,
    onRows,
    toolbar,
    empty: emptySlot,
    className,
    classNames,
    ...hookOptions
  } = props;

  const grid = useEntityGrid(hookOptions);
  const { rows, columns, loading, error, labels, state, cellOf, toggleMeta, setOffset, setSort, setFilter, setFilterText, filterColumns, askedQuery, setEdit, clearEdits, pendingChanges, reload, getGridProps, getHeaderProps, getRowProps, getCellProps } = grid;

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
  // Edit mode needs a place to write: without one the grid is a reader, whatever the config says.
  // A source that takes no writes switches the feature off, whatever the config says (SDK-29).
  const editing = hookOptions.config.mode === "edit" && Boolean(hookOptions.source.patch);
  // Which columns open: the config's own list, which `parseGridConfig` requires in edit mode, so a
  // grid never opens a column nobody named.
  const editable = useMemo(() => {
    const allowed = hookOptions.config.editableAttrs;
    return (column: VisibleColumn): boolean =>
      // The attribute's own column only: a metadata column shows when the value was observed or
      // what it is measured in, and neither is corrected by typing over it.
      editing && column.attr !== null && column.meta === null && allowed.includes(column.attr);
  }, [editing, hookOptions.config.editableAttrs]);
  const [open, setOpen] = useState(false);
  const [observed, setObserved] = useState<Observed>("keep");
  const [applying, setApplying] = useState(false);
  const [history, setHistory] = useState<{ attr: string; id: string } | null>(null);
  const [refused, setRefused] = useState<Refusal[]>([]);
  const refusedOf = useMemo(() => new Map(refused.map((one) => [one.id, one.detail])), [refused]);
  const rowTitle = useCallback(
    (id: string): string | undefined => {
      const detail = refusedOf.get(id);
      return detail ? `${labels.refusedHere}: ${detail}` : undefined;
    },
    [refusedOf, labels.refusedHere],
  );

  /**
   * Applies the pending cells through the endpoint. What landed is forgotten and re-read, so the
   * grid shows the endpoint's own answer; what was refused stays pending with its reason, because
   * the person's value is the only copy of it (UI-67).
   */
  const apply = useCallback(async () => {
    setApplying(true);
    try {
      const result = await applyChanges({
        source: hookOptions.source,
        entities: pendingChanges,
        observed,
        fallback: labels.error,
      });
      for (const id of result.applied) {
        for (const attr of Object.keys(state.edits[id] ?? {})) {
          setEdit(id, attr, undefined);
        }
      }
      setRefused(result.refused);
      if (result.refused.length === 0) {
        setOpen(false);
      }
      reload();
    } finally {
      setApplying(false);
    }
  }, [hookOptions.source, pendingChanges, observed, labels.error, state.edits, setEdit, reload]);

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
    // The identifier column carries no attribute, so a host addresses it by its key.
    if (column.attr === null && renderers?.[column.key]) {
      return renderers[column.key](cell, row);
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

    if (column.attr && editable(column)) {
      const pending = state.edits[row.id]?.[column.attr];
      const one = Array.isArray(cell) ? cell[0] : cell;
      // The value itself, not the text the cell shows: a person edits `5`, not `5 C62` — the unit
      // is what the value is measured in and is kept, never typed over.
      const own = one?.kind === "relationship" ? one.object : one?.value;
      const shown = own === undefined || own === null ? "" : String(own);
      return (
        <EditableCell
          label={`${labels.edit} ${column.label}`}
          value={pending === undefined ? shown : String(pending)}
          changed={pending !== undefined}
          kind={typeof own === "number" ? "number" : "text"}
          onChange={(next) => {
            // Back to the endpoint's own value is not a change: it leaves the pending list.
            setEdit(row.id, column.attr!, next === shown ? undefined : coerce(next, shown));
          }}
        />
      );
    }

    return <>{text}</>;
  }, [cellOf, renderers, onOpenRelationship, editable, state.edits, labels.edit, setEdit]);

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

  // What the source answered, handed to the host as it arrives: the page a person is looking at is
  // what they mean by "this page" when they export it.
  React.useEffect(() => {
    onRows?.(rows, state.offset);
  }, [rows, state.offset, onRows]);

  const rootClass = `jc-grid${className ? ` ${className}` : ""}${classNames?.root ? ` ${classNames.root}` : ""}`;

  return (
    <div className={rootClass} data-density={hookOptions.config.density}>
      {toolbar && <div className="jc-grid-toolbar">{toolbar}</div>}

      {editing && pendingChanges.length > 0 && (
        <div className="jc-grid-pending" role="status">
          <span>{`${pendingChanges.length} ${labels.pending}`}</span>
          <button type="button" onClick={() => setOpen(true)}>
            {labels.review}
          </button>
          <button type="button" onClick={() => { clearEdits(); setRefused([]); }}>
            {labels.discard}
          </button>
        </div>
      )}

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
                      {/* How this attribute got to its value, where its metadata already is: the
                          entry is absent when the source cannot answer a temporal read or the
                          config switched history off (T-1431). */}
                      {hookOptions.source.history && hookOptions.config.history.enabled !== false && (
                        <button
                          type="button"
                          className="jc-grid-meta-item"
                          onClick={() => {
                            // The row the person is on, else the first: history belongs to one
                            // entity, and the panel names which.
                            setHistory({
                              attr: col.attr!,
                              id: (rows[state.activeCell?.row ?? 0] ?? rows[0])?.id ?? "",
                            });
                            setOpenMenu(null);
                          }}
                        >
                          {labels.history}
                        </button>
                      )}
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
              <tr
                key={row.id}
                className={`jc-grid-tr${classNames?.row ? ` ${classNames.row}` : ""}`}
                // A refused update stays visible on its own row, not only in the panel the person
                // may have closed: the value there is still theirs and still unapplied.
                data-refused={refusedOf.has(row.id) ? "true" : undefined}
                title={rowTitle(row.id)}
                {...getRowProps(row, rowIndex)}
              >
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

      {history && (
        <EntityHistory
          source={hookOptions.source}
          id={history.id}
          attr={history.attr}
          maxPoints={hookOptions.config.history.maxPoints}
          labels={hookOptions.labels?.historyLabels}
          onClose={() => setHistory(null)}
        />
      )}

      {open && (
        <section className="jc-grid-review" aria-label={labels.review}>
          <table>
            <thead>
              <tr>
                <th>{labels.id}</th>
                <th>{labels.filter}</th>
                <th>{labels.observedKeep}</th>
                <th>{labels.apply}</th>
              </tr>
            </thead>
            <tbody>
              {pendingChanges.flatMap((entity) =>
                entity.changes.map((change) => (
                  <tr key={`${entity.id}-${change.attribute}`}>
                    <td title={entity.id}>{entity.id}</td>
                    <td>{change.attribute}</td>
                    <td>{change.before === undefined ? "—" : String(change.before)}</td>
                    <td>{String(change.after)}</td>
                  </tr>
                )),
              )}
            </tbody>
          </table>
          {/* What happens to `observedAt`, chosen once for the batch: a corrected value that keeps
              the old timestamp claims to have been observed then (UI-67). */}
          <fieldset>
            <label>
              <input
                type="radio"
                name="jc-grid-observed"
                checked={observed === "keep"}
                onChange={() => setObserved("keep")}
              />
              {labels.observedKeep}
            </label>
            <label>
              <input
                type="radio"
                name="jc-grid-observed"
                checked={observed === "now"}
                onChange={() => setObserved("now")}
              />
              {labels.observedNow}
            </label>
          </fieldset>
          {pendingChanges.length > MAX_ENTITIES && (
            <p className="jc-grid-error">{`${labels.error}: ${MAX_ENTITIES}`}</p>
          )}
          {refused.map((one) => (
            <p key={one.id} className="jc-grid-error">{`${one.id}: ${one.detail}`}</p>
          ))}
          <button
            type="button"
            disabled={applying || pendingChanges.length > MAX_ENTITIES}
            onClick={() => void apply()}
          >
            {applying ? labels.applying : labels.apply}
          </button>
        </section>
      )}

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

/**
 * A value typed into a cell, in the shape the cell had: a column of numbers stays numbers, so the
 * endpoint is not sent a string where it stored a measurement.
 */
function coerce(next: string, before: string): unknown {
  if (before !== "" && Number.isFinite(Number(before)) && Number.isFinite(Number(next))) {
    return Number(next);
  }
  if (next === "true" || next === "false") {
    return next === "true";
  }
  return next;
}

/** One cell a person may correct: an input that says what it belongs to, marked while pending. */
function EditableCell({
  label,
  value,
  changed,
  kind,
  onChange,
}: {
  label: string;
  value: string;
  changed: boolean;
  kind: "text" | "number";
  onChange: (next: string) => void;
}): React.JSX.Element {
  return (
    <input
      aria-label={label}
      className={`jc-grid-cell-input${changed ? " jc-grid-cell-changed" : ""}`}
      data-changed={changed ? "true" : undefined}
      type={kind}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
