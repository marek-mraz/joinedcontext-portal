import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { edit, parseModel } from "./linkml";
import type { Artifacts } from "./LinkmlPreviewPanel";

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
  /** Hands the adapted LinkML source to the editor, with the model it came from. */
  onImport: (source: string, model: CatalogueModel) => void;
}

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

const INPUT =
  "rounded border border-border bg-surface px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus";

export function SmartDataModelsImport({ onImport }: SmartDataModelsImportProps): JSX.Element {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [subject, setSubject] = useState("");
  const [selected, setSelected] = useState<CatalogueModel | null>(null);
  const [refreshes, setRefreshes] = useState(0);
  const [keep, setKeep] = useState<string[] | null>(null);

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
  const upstreamSlots = useMemo(
    () => (imported ? parseModel(imported).slots.map((slot) => slot.name) : []),
    [imported],
  );
  const chosen = keep ?? upstreamSlots;

  const doImport = () => {
    if (!imported || !selected) {
      return;
    }
    onImport(deprecateUnused(imported, chosen), selected);
  };

  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <section aria-labelledby="sdm-browse" className="flex flex-col gap-3">
        <h3 id="sdm-browse" className="text-base font-semibold">
          {t("models.sdm.browse")}
        </h3>
        <div className="flex flex-wrap gap-2">
          <input
            className={`${INPUT} w-56`}
            type="search"
            aria-label={t("models.sdm.search")}
            placeholder={t("models.sdm.search")}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <select
            className={INPUT}
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
          </select>
          <button
            type="button"
            className="rounded border border-border px-2 py-1 text-sm"
            onClick={() => setRefreshes((count) => count + 1)}
          >
            {t("models.sdm.refresh")}
          </button>
        </div>

        {catalogue.isError ? (
          <p role="status" className="text-sm text-warning-fg">
            {t("models.sdm.unavailable")}
          </p>
        ) : null}
        {catalogue.data?.stale ? (
          <p role="status" className="text-sm text-warning-fg">
            {t("models.sdm.stale", { at: catalogue.data.refreshedAt ?? "—" })}
          </p>
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
                      className={
                        selected?.id === model.id
                          ? "w-full rounded bg-surface-subtle px-2 py-1 text-left text-sm font-medium"
                          : "w-full rounded px-2 py-1 text-left text-sm hover:bg-surface-subtle"
                      }
                    >
                      <span className="block">{model.name}</span>
                      {model.description ? (
                        <span className="block text-xs text-surface-fg/70">
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
        <h3 id="sdm-preview" className="text-base font-semibold">
          {t("models.sdm.preview")}
        </h3>
        {!selected ? (
          <p className="text-sm text-surface-fg/70">{t("models.sdm.pick")}</p>
        ) : preview.isError ? (
          <p role="status" className="text-sm text-warning-fg">
            {t("models.sdm.previewFailed")}
          </p>
        ) : imported ? (
          <>
            <p className="text-sm">{t("models.sdm.keepAll")}</p>
            <ul className="flex max-h-56 flex-col gap-1 overflow-auto text-sm">
              {upstreamSlots.map((name) => (
                <li key={name}>
                  <label className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={chosen.includes(name)}
                      onChange={(event) =>
                        setKeep(
                          event.target.checked
                            ? [...chosen, name]
                            : chosen.filter((slot) => slot !== name),
                        )
                      }
                    />
                    {name}
                  </label>
                </li>
              ))}
            </ul>
            <pre className="max-h-56 overflow-auto rounded border border-border bg-surface-subtle p-3 text-xs">
              {imported}
            </pre>
            <button
              type="button"
              className="self-start rounded bg-primary px-3 py-1.5 text-sm font-medium text-primary-fg"
              onClick={doImport}
            >
              {t("models.sdm.import", { name: selected.name })}
            </button>
          </>
        ) : (
          <p className="text-sm text-surface-fg/70">{t("models.sdm.loading")}</p>
        )}
      </section>
    </div>
  );
}
