import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, unwrap } from "../../api/client";
import { edit, parseModel } from "./linkml";
import type { Artifacts } from "./LinkmlPreviewPanel";
import { Alert, Button, Input, Select } from "../../components/ui";

/**
 * The primary path into a model: take an official Smart Data Model and adapt it (DM-07).
 *
 * The browser never reaches the catalogue. It asks the Portal, the Portal asks Model Tools, and
 * Model Tools is the only component with an allowlist that lets it fetch from the Smart Data
 * Models organisation at all (DM-10). The index it answers with is a daily cache; when a
 * refresh could not reach the catalogue the older index is shown as stale rather than withheld,
 * because an editor that cannot browse must still be able to work (DM-12).
 */
export interface CatalogueModel {
  id: string;
  name: string;
  description?: string;
  attributes?: string[];
}

export interface CatalogueSubject {
  name: string;
  title?: string;
  models?: CatalogueModel[];
}

export interface Catalogue {
  subjects?: CatalogueSubject[];
  refreshedAt?: string;
  stale?: boolean;
}

export interface SmartDataModelsImportProps {
  /**
   * Hands the adapted LinkML source to the editor, with the model it came from and the space
   * the person chose for it, when they chose one (DM-57).
   */
  onImport: (source: string, model: CatalogueModel, space?: string) => void;
  /** The project's Context Spaces, so the model can be placed before it is imported (T-1108). */
  spaces?: string[];
}

/** How many attributes the panel shows before "show the rest": a list, not a scroll box. */
const ATTRIBUTE_PAGE = 50;

/** Whether a model answers the search, by name, description or attribute name. */
export function matches(model: CatalogueModel, needle: string): boolean {
  const query = needle.trim().toLowerCase();
  if (!query) {
    return true;
  }
  return (
    model.name.toLowerCase().includes(query) ||
    (model.description ?? "").toLowerCase().includes(query) ||
    (model.attributes ?? []).some((attribute) => attribute.toLowerCase().includes(query))
  );
}

/**
 * The imported source with the slots the user did not pick marked deprecated (DM-11).
 *
 * Deleting them would make a federation partner's payload invalid against our own model, so an
 * unused upstream slot stays in the schema and says it is not the one to use.
 */
export function deprecateUnused(source: string, keep: string[]): string {
  const model = parseModel(source);
  const kept = new Set(keep);
  return edit(source, (document) => {
    for (const slot of model.slots) {
      if (!kept.has(slot.name)) {
        document.setIn(["slots", slot.name, "deprecated"], true);
      }
    }
  });
}

export function SmartDataModelsImport({
  onImport,
  spaces = [],
}: SmartDataModelsImportProps): JSX.Element {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [subject, setSubject] = useState("");
  const [selected, setSelected] = useState<CatalogueModel | null>(null);
  const [refreshes, setRefreshes] = useState(0);
  const [keep, setKeep] = useState<string[] | null>(null);
  /** What the person typed to find an attribute among the two hundred a model may have. */
  const [attribute, setAttribute] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [space, setSpace] = useState("");

  const catalogue = useQuery({
    queryKey: ["tools", "sdm-catalog", refreshes],
    retry: false,
    queryFn: async (): Promise<Catalogue> =>
      unwrap(
        await api.GET("/api/v1/tools/sdm-catalog", {
          params: { query: refreshes > 0 ? { refresh: true } : {} },
        }),
      ) as Catalogue,
  });

  const preview = useQuery({
    queryKey: ["tools", "import-sdm", selected?.id],
    enabled: Boolean(selected),
    retry: false,
    staleTime: Infinity,
    queryFn: async (): Promise<Artifacts> =>
      unwrap(
        await api.POST("/api/v1/tools/import-sdm", { body: { model: selected?.id ?? "" } }),
      ) as Artifacts,
  });

  const subjects = useMemo(() => catalogue.data?.subjects ?? [], [catalogue.data]);
  const visible = useMemo(
    () =>
      subjects
        .filter((entry) => !subject || entry.name === subject)
        .map((entry) => ({
          ...entry,
          models: (entry.models ?? []).filter((model) => matches(model, search)),
        }))
        .filter((entry) => entry.models.length > 0),
    [subjects, subject, search],
  );

  const imported = preview.data?.linkml;
  const model = useMemo(() => (imported ? parseModel(imported) : undefined), [imported]);
  const upstreamSlots = useMemo(() => (model?.slots ?? []).map((slot) => slot.name), [model]);
  const chosen = keep ?? upstreamSlots;

  // A model of the catalogue can carry two hundred attributes, which is a scroll box nobody
  // reads (T-1107). What is required comes first, what the person typed narrows it, and the
  // rest waits behind one button rather than filling the panel.
  const offered = useMemo(() => {
    const needle = attribute.trim().toLowerCase();
    const matching = (model?.slots ?? []).filter((slot) =>
      needle === "" ? true : slot.name.toLowerCase().includes(needle),
    );
    const required = matching.filter((slot) => slot.required === true);
    const optional = matching.filter((slot) => slot.required !== true);
    return { required, optional, total: matching.length };
  }, [model, attribute]);
  const shown = showAll
    ? [...offered.required, ...offered.optional]
    : [...offered.required, ...offered.optional].slice(0, ATTRIBUTE_PAGE);
  const hidden = offered.total - shown.length;

  const doImport = () => {
    if (!imported || !selected) {
      return;
    }
    onImport(deprecateUnused(imported, chosen), selected, space || undefined);
  };

  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <section aria-labelledby="sdm-browse" className="flex flex-col gap-3">
        <h2 id="sdm-browse" className="text-base font-semibold">
          {t("models.sdm.browse")}
        </h2>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            className="w-56"
            type="search"
            aria-label={t("models.sdm.search")}
            placeholder={t("models.sdm.search")}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <Select
            aria-label={t("models.sdm.subject")}
            value={subject}
            onChange={(event) => setSubject(event.target.value)}
          >
            <option value="">{t("models.sdm.allSubjects")}</option>
            {subjects.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.title ?? entry.name}
              </option>
            ))}
          </Select>
          <Button
            size="sm"
            onClick={() => setRefreshes((count) => count + 1)}
          >
            {t("models.sdm.refresh")}
          </Button>
        </div>

        {catalogue.isError ? (
          <Alert tone="danger" role="status">
            {t("models.sdm.unavailable")}
          </Alert>
        ) : null}
        {catalogue.data?.stale ? (
          <Alert tone="warning" role="status">
            {t("models.sdm.stale", { at: catalogue.data.refreshedAt ?? "—" })}
          </Alert>
        ) : null}

        <ul className="flex flex-col gap-3">
          {visible.map((entry) => (
            <li key={entry.name}>
              <h4 className="text-sm font-semibold">{entry.title ?? entry.name}</h4>
              <ul className="mt-1 flex flex-col gap-1">
                {entry.models.map((model) => (
                  <li key={model.id}>
                    <button
                      type="button"
                      onClick={() => {
                        setSelected(model);
                        setKeep(null);
                      }}
                      aria-current={selected?.id === model.id ? "true" : undefined}
                      className={clsx(
                        "focus-ring w-full rounded-md px-2.5 py-1.5 text-left text-body transition-colors",
                        selected?.id === model.id
                          ? "bg-surface-muted font-medium text-fg"
                          : "hover:bg-surface-subtle text-fg",
                      )}
                    >
                      <span className="block font-medium">{model.name}</span>
                      {model.description ? (
                        <span className="block text-caption text-fg-muted">
                          {model.description}
                        </span>
                      ) : null}
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
        {!catalogue.isLoading && visible.length === 0 ? (
          <p className="text-sm text-surface-fg/70">{t("models.sdm.noMatches")}</p>
        ) : null}
      </section>

      <section aria-labelledby="sdm-preview" className="flex flex-col gap-3">
        <h2 id="sdm-preview" className="text-base font-semibold">
          {t("models.sdm.preview")}
        </h2>
        {!selected ? (
          <p className="text-body text-fg-muted">{t("models.sdm.pick")}</p>
        ) : preview.isError ? (
          // Model Tools timing out is the ordinary failure here, and selecting the model again
          // to retry is a step nobody should have to know about (T-1109).
          <Alert
            tone="danger"
            role="status"
            actions={
              <Button size="sm" onClick={() => void preview.refetch()}>
                {t("app.error.retry")}
              </Button>
            }
          >
            {t("models.sdm.previewFailed")}
          </Alert>
        ) : imported ? (
          <>
            <p className="text-body">{t("models.sdm.keepAll")}</p>
            <Input
              className="w-full"
              type="search"
              aria-label={t("models.sdm.findAttribute")}
              placeholder={t("models.sdm.findAttribute")}
              value={attribute}
              onChange={(event) => {
                setAttribute(event.target.value);
                setShowAll(false);
              }}
            />
            <ul className="flex max-h-56 flex-col gap-1 overflow-auto text-body">
              {shown.map((slot) => (
                <li key={slot.name}>
                  <label className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={chosen.includes(slot.name)}
                      onChange={(event) =>
                        setKeep(
                          event.target.checked
                            ? [...chosen, slot.name]
                            : chosen.filter((name) => name !== slot.name),
                        )
                      }
                    />
                    {slot.name}
                    {slot.required ? (
                      <span className="text-caption text-fg-muted">
                        {t("models.sdm.requiredAttribute")}
                      </span>
                    ) : null}
                  </label>
                </li>
              ))}
            </ul>
            {offered.total === 0 ? (
              <p className="text-caption text-fg-muted">{t("models.sdm.noAttribute")}</p>
            ) : null}
            {hidden > 0 ? (
              <Button size="sm" className="self-start" onClick={() => setShowAll(true)}>
                {t("models.sdm.showMore", { count: hidden })}
              </Button>
            ) : null}
            {spaces.length > 0 ? (
              <label className="flex flex-col gap-1 text-body">
                {t("models.sdm.space")}
                <Select value={space} onChange={(event) => setSpace(event.target.value)}>
                  <option value="">{t("models.sdm.spaceLater")}</option>
                  {spaces.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </Select>
              </label>
            ) : null}
            <pre className="max-h-56 overflow-auto rounded border border-border bg-surface-subtle p-3 font-mono text-xs text-fg">
              {imported}
            </pre>
            <Button
              variant="primary"
              size="sm"
              className="self-start"
              onClick={doImport}
            >
              {t("models.sdm.import", { name: selected.name })}
            </Button>
          </>
        ) : (
          <p className="text-body text-fg-muted">{t("models.sdm.loading")}</p>
        )}
      </section>
    </div>
  );
}
