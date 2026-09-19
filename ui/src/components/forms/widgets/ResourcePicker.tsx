import { useMemo } from "react";
import type { ChangeEvent, FocusEvent, JSX } from "react";
import type { WidgetProps } from "@rjsf/utils";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { ApiError, api, queryKeys, unwrap } from "../../../api/client";
import { asManifests, localized } from "../../../api/manifest";

/**
 * A parameter whose choices are the project's own manifests of one kind (CC-24).
 *
 * The schema names the plural segment of `/api/v1/projects/{project}/{plural}`; the chosen
 * manifest's name is the value. The list comes back already narrowed to what the caller may
 * read, so the picker shows nobody an object they could not open themselves, and the server
 * validates the value against the same schema either way.
 */
export function ResourcePicker(props: WidgetProps): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, onBlur, onFocus, options, rawErrors } =
    props;
  const { t, i18n } = useTranslation();

  const plural = typeof options?.plural === "string" ? options.plural : undefined;
  // Written into the uiSchema by the page that renders the form: a widget has no route of
  // its own, and a parameter schema must not be able to name another project.
  const project = typeof options?.project === "string" ? options.project : undefined;

  const query = useQuery({
    queryKey: queryKeys.list(project ?? "", plural ?? ""),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: project as string, plural: plural as string } },
        }),
      ),
    enabled: Boolean(project && plural),
  });

  const choices = useMemo(
    () =>
      asManifests(query.data?.items ?? []).map((manifest) => ({
        name: manifest.metadata.name,
        label: localized(manifest.metadata.title, i18n.language, manifest.metadata.name),
      })),
    [query.data, i18n.language],
  );

  // A list that could not be asked for is not an empty project (UI-44, T-1486): a 403 or a 500
  // read as "No results" in every form that picks a resource, so nobody could tell the difference.
  const failed = query.isError;
  const reason =
    query.error instanceof ApiError
      ? (query.error.problem?.detail ?? query.error.message)
      : query.error instanceof Error
        ? query.error.message
        : "";
  const empty = query.isLoading
    ? t("app.loading")
    : failed
      ? t("form.noResults")
      : choices.length === 0
        ? t("form.noResults")
        : t("form.choose");

  const select = (
    <select
      id={id}
      required={required}
      disabled={disabled || readonly}
      aria-invalid={rawErrors && rawErrors.length > 0 ? "true" : undefined}
      value={typeof value === "string" ? value : ""}
      onChange={(event: ChangeEvent<HTMLSelectElement>) =>
        onChange(event.target.value === "" ? undefined : event.target.value)
      }
      onBlur={(event: FocusEvent<HTMLSelectElement>) => onBlur?.(id, event.target.value)}
      onFocus={(event: FocusEvent<HTMLSelectElement>) => onFocus?.(id, event.target.value)}
      className={clsx(
        "block w-full rounded border border-border bg-surface px-3 py-1.5 text-base text-surface-fg focus:border-border-focus focus:outline-none focus:ring-2 focus:ring-border-focus",
        (disabled || readonly) && "cursor-not-allowed opacity-50",
      )}
    >
      <option value="">{empty}</option>
      {choices.map((choice) => (
        <option key={choice.name} value={choice.name}>
          {choice.label}
        </option>
      ))}
    </select>
  );

  if (!failed) {
    return select;
  }
  return (
    <div className="flex flex-col gap-1">
      {select}
      <p id={`${id}-error`} role="alert" className="text-caption text-danger">
        {t("form.listFailed", { reason })}{" "}
        <button
          type="button"
          className="underline hover:no-underline"
          onClick={() => {
            void query.refetch();
          }}
        >
          {t("form.listRetry")}
        </button>
      </p>
    </div>
  );
}
