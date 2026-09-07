import type { HTMLAttributes, ReactNode, TdHTMLAttributes, ThHTMLAttributes } from "react";
import { clsx } from "clsx";
import { Skeleton } from "./Skeleton";

export interface TableProps extends HTMLAttributes<HTMLTableElement> {
  /** Read by a screen reader as the table's name; visually hidden. */
  caption: string;
  /** Every second row tinted; on by default. */
  zebra?: boolean;
  /** Announced to a screen reader while the rows are a skeleton: "Loading endpoints". */
  status?: string;
}

/**
 * A data table inside a scrolling frame: the frame scrolls on a phone while the page stays
 * put, the header stays legible, and the caption names it for a screen reader (UI-01).
 */
export function Table({
  caption,
  zebra = true,
  status,
  className,
  children,
  ...rest
}: TableProps): React.JSX.Element {
  return (
    <div className="overflow-x-auto rounded-lg border border-border bg-surface shadow-1">
      {status ? (
        <p role="status" className="sr-only">
          {status}
        </p>
      ) : null}
      <table
        className={clsx(
          "w-full border-collapse text-left text-body",
          zebra && "[&>tbody>tr:nth-child(even)]:bg-surface-subtle/70",
          className,
        )}
        {...rest}
      >
        <caption className="sr-only">{caption}</caption>
        {children}
      </table>
    </div>
  );
}

export function TableHead({
  className,
  children,
  ...rest
}: HTMLAttributes<HTMLTableSectionElement>): React.JSX.Element {
  return (
    <thead className={clsx("bg-surface-subtle", className)} {...rest}>
      <tr className="border-b border-border">{children}</tr>
    </thead>
  );
}

export function TableBody({
  className,
  children,
  ...rest
}: HTMLAttributes<HTMLTableSectionElement>): React.JSX.Element {
  return (
    <tbody className={clsx("divide-y divide-border", className)} {...rest}>
      {children}
    </tbody>
  );
}

export function TableRow({
  className,
  children,
  ...rest
}: HTMLAttributes<HTMLTableRowElement>): React.JSX.Element {
  return (
    <tr className={clsx("transition-colors hover:bg-primary-50", className)} {...rest}>
      {children}
    </tr>
  );
}

export interface TableHeaderCellProps extends ThHTMLAttributes<HTMLTableCellElement> {
  align?: "left" | "right" | "center";
}

export function TableHeaderCell({
  align = "left",
  className,
  children,
  ...rest
}: TableHeaderCellProps): React.JSX.Element {
  return (
    <th
      scope="col"
      className={clsx(
        "whitespace-nowrap px-4 py-2.5 text-caption font-semibold uppercase tracking-wide text-fg-muted",
        align === "right" && "text-right",
        align === "center" && "text-center",
        className,
      )}
      {...rest}
    >
      {children}
    </th>
  );
}

export interface TableCellProps extends TdHTMLAttributes<HTMLTableCellElement> {
  align?: "left" | "right" | "center";
  /** The cell's content is the row's name: bolder, and first to be read. */
  primary?: boolean;
}

export function TableCell({
  align = "left",
  primary,
  className,
  children,
  ...rest
}: TableCellProps): React.JSX.Element {
  return (
    <td
      className={clsx(
        "px-4 py-3 align-top text-fg",
        primary && "font-medium",
        align === "right" && "text-right",
        align === "center" && "text-center",
        className,
      )}
      {...rest}
    >
      {children}
    </td>
  );
}

/** The one row of an empty table: the message, and whatever action fills it. */
export function TableEmpty({
  columns,
  children,
}: {
  columns: number;
  children: ReactNode;
}): React.JSX.Element {
  return (
    <tr>
      <td colSpan={columns} className="px-4 py-12 text-center text-body text-fg-muted">
        {children}
      </td>
    </tr>
  );
}

/** Rows of grey bars while the list is on its way; the Table's `status` announces the wait. */
export function TableSkeleton({
  columns,
  rows = 4,
}: {
  columns: number;
  rows?: number;
}): React.JSX.Element {
  return (
    <tbody aria-hidden="true" className="divide-y divide-border">
      {Array.from({ length: rows }, (_, row) => (
        <tr key={row}>
          {Array.from({ length: columns }, (_, col) => (
            <td key={col} className="px-4 py-3.5">
              <Skeleton className={col === 0 ? "h-4 w-40" : "h-4 w-24"} />
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  );
}
