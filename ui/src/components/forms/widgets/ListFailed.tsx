import { useTranslation } from "react-i18next";

/**
 * Why a choice is empty: the list could not be asked for (UI-44, T-1502).
 *
 * An empty select and a select whose list failed look the same, and they mean opposite things —
 * "there is nothing here yet" against "nobody could ask". One line in the same words everywhere,
 * with the API's own reason and a retry, so a person is not left guessing which of the two it is.
 * The retry is the query's `refetch`, because a failed list is worth one more try before a reload.
 */
export function ListFailed({
  what,
  reason,
  onRetry,
  id,
}: {
  /** The list in the person's own words, e.g. "Data sources". */
  what: string;
  /** The API's problem detail, and nothing else: no stack, no URL, no token. */
  reason: string;
  onRetry?: () => void;
  id?: string;
}): React.JSX.Element {
  const { t } = useTranslation();
  return (
    <p id={id} role="alert" className="text-caption text-danger">
      {what}: {t("form.listFailed", { reason })}{" "}
      {onRetry ? (
        <button type="button" className="underline hover:no-underline" onClick={onRetry}>
          {t("form.listRetry")}
        </button>
      ) : null}
    </p>
  );
}

/** The sentence an API error carries, or the generic one: never an object a person cannot read. */
export function reasonOf(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim() !== "") {
    return error.message;
  }
  return fallback;
}
