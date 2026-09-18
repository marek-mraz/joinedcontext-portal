import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ApiError } from "../api/client";
import { Alert, Button, Icon, Table, TableBody, TableEmpty, TableSkeleton } from "./ui";

/**
 * The table of a resource list page (T-1382): a skeleton while the list loads, the API's own
 * reason with a retry when it fails, the empty state when it holds nothing, the rows otherwise.
 * The page keeps its header, its actions and what it draws around the table.
 */
export function ResourceList({
  query,
  caption,
  head,
  columns,
  count,
  empty,
  children,
}: {
  query: { isPending: boolean; isError: boolean; error: unknown; refetch: () => unknown };
  caption: string;
  head: ReactNode;
  columns: number;
  /** How many rows the list holds; none shows `empty`. */
  count: number;
  /** An `EmptyState`, `bare`, with the page's create action when it has one (T-1381). */
  empty: ReactNode;
  children: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  if (query.isError) {
    const error = query.error;
    const message =
      error instanceof ApiError ? (error.problem?.detail ?? error.message) : t("app.error.generic");
    return (
      <Alert
        role="alert"
        tone="danger"
        actions={
          <Button
            size="sm"
            icon={<Icon name="refresh" className="size-4" />}
            onClick={() => {
              void query.refetch();
            }}
          >
            {t("app.error.retry")}
          </Button>
        }
      >
        {message}
      </Alert>
    );
  }
  return (
    <Table caption={caption} status={query.isPending ? t("app.loading") : undefined}>
      {head}
      {query.isPending ? (
        <TableSkeleton columns={columns} />
      ) : (
        <TableBody>
          {count === 0 ? <TableEmpty columns={columns}>{empty}</TableEmpty> : children}
        </TableBody>
      )}
    </Table>
  );
}
