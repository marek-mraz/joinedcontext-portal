import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, refName } from "../../api/manifest";
import { LinkmlPreviewPanel } from "./LinkmlPreviewPanel";
import { LinkmlSourceEditor } from "./LinkmlSourceEditor";
import { LinkmlVisualEditor } from "./LinkmlVisualEditor";
import { MappingsEditor } from "./MappingsEditor";
import type { MappingModel } from "./MappingsEditor";
import { SmartDataModelsImport } from "./SmartDataModelsImport";
import type { CatalogueModel } from "./SmartDataModelsImport";
import { blankSource, diagnose, parseModel } from "./linkml";
import {
  bumpVersion,
  classifyChanges,
  laneOf,
  refusalToSave,
  severityOf,
} from "./breaking_detector";
import type { Lifecycle } from "./breaking_detector";

/**
 * The LinkML editor: one document, four ways of working on it (DM-13, DM-17, DM-23).
 *
 * The document lives here as text and nowhere else. The structured view and the source view
 * both edit that one string, the preview compiles it, and the import wizard replaces it, so
 * the views cannot hold different models. Nothing on this page writes to the platform: saving
 * a model is a repository change, which is the ordinary lane flow (CC-32, CC-63).
 */
export interface ModelsPageProps {
  project: string;
  /** The organisation's locales, for the language maps every title needs (DM-15). */
  locales?: string[];
  /**
   * The version this edit started from. The import wizard sets it; a future task sets it from
   * the `DataModel` manifest, which is where a published model's source and version live.
   */
  baseline?: { source: string; version: string; lifecycle: Lifecycle; name: string };
  /**
   * The other models this one can be mapped to and from (DM-33). The document being edited is
   * always among them, so a mapping can be written before it is published.
   */
  mappable?: MappingModel[];
}

type Tab = "structure" | "source" | "preview" | "import" | "mappings";

const TABS: Tab[] = ["import", "structure", "source", "preview", "mappings"];

export function ModelsPage({
  project,
  locales = ["sk", "en", "de", "cs"],
  baseline,
  mappable = [],
}: ModelsPageProps): JSX.Element {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>(baseline ? "structure" : "import");
  const [published, setPublished] = useState(baseline);
  const [source, setSource] = useState(
    baseline?.source ?? blankSource(`${project}.sk`, "new-model"),
  );

  const model = useMemo(() => parseModel(source), [source]);
  const diagnostics = useMemo(() => diagnose(source, locales), [source, locales]);

  const changes = useMemo(
    () => (published ? classifyChanges(parseModel(published.source), model) : []),
    [published, model],
  );
  const severity = severityOf(changes);
  const nextVersion = published ? bumpVersion(published.version, changes) : "1.0.0";
  const lane = laneOf(changes, published?.lifecycle ?? "draft");
  const refusal = published
    ? refusalToSave(changes, published.version, published.version)
    : undefined;

  // Who breaks when this model's major changes (DM-25). A space names its model, and an
  // endpoint names its space, so the consumers of a model are the endpoints behind it.
  const listOf = async (plural: string) => {
    const answer = await unwrap(
      await api.GET("/api/v1/projects/{project}/{plural}", {
        params: { path: { project, plural } },
      }),
    );
    return asManifests(answer.items ?? []);
  };
  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    retry: false,
    queryFn: () => listOf("spaces"),
  });
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    retry: false,
    queryFn: () => listOf("endpoints"),
  });

  const consumers = useMemo(() => {
    const name = published?.name ?? model.name;
    if (!name) {
      return [];
    }
    const servingSpaces = (spaces.data ?? [])
      .filter((space) => (space.spec as { dataModelRef?: string })?.dataModelRef === name)
      .map((space) => space.metadata.name);
    const servingEndpoints = (endpoints.data ?? [])
      .filter((endpoint) =>
        servingSpaces.includes(
          refName((endpoint.spec as { contextSpaceRef?: unknown })?.contextSpaceRef),
        ),
      )
      .map((endpoint) => `Endpoint/${endpoint.metadata.name}`);
    return [...servingSpaces.map((space) => `ContextSpace/${space}`), ...servingEndpoints];
  }, [spaces.data, endpoints.data, published, model.name]);

  const onImport = (imported: string, catalogueModel: CatalogueModel) => {
    setSource(imported);
    setPublished({
      source: imported,
      version: "1.0.0",
      lifecycle: "draft",
      name: catalogueModel.name,
    });
    setTab("structure");
  };

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-xl font-semibold">{t("models.title")}</h2>
        <p className="text-sm text-surface-fg/70">
          {t("models.version", { version: nextVersion })}
        </p>
      </header>

      {published ? (
        <section
          aria-labelledby="models-changes"
          className={
            severity === "breaking"
              ? "rounded border border-danger p-3"
              : "rounded border border-border p-3"
          }
        >
          <h3 id="models-changes" className="text-sm font-semibold">
            {t(`models.severity.${severity}`)} · {t(`lane.${lane}`)}
          </h3>
          {refusal ? (
            <p role="alert" className="mt-1 text-sm text-danger-fg">
              {refusal}
            </p>
          ) : null}
          {changes.length > 0 ? (
            <ul className="mt-2 flex flex-col gap-1 text-sm">
              {changes.map((change) => (
                <li key={`${change.subject}-${change.reason}`}>
                  <span className="font-mono text-xs">{change.subject}</span> {change.reason}
                </li>
              ))}
            </ul>
          ) : (
            <p className="mt-1 text-sm text-surface-fg/70">{t("models.noChanges")}</p>
          )}
          {severity === "breaking" && consumers.length > 0 ? (
            <p className="mt-2 text-sm">
              {t("models.consumers", { consumers: consumers.join(", ") })}
            </p>
          ) : null}
        </section>
      ) : null}

      <div role="tablist" aria-label={t("models.title")} className="flex flex-wrap gap-1">
        {TABS.map((name) => (
          <button
            key={name}
            type="button"
            role="tab"
            aria-selected={tab === name}
            onClick={() => setTab(name)}
            className={
              tab === name
                ? "rounded border border-border bg-surface-subtle px-3 py-1 text-sm font-medium"
                : "rounded border border-transparent px-3 py-1 text-sm hover:bg-surface-subtle"
            }
          >
            {t(`models.view.${name}`)}
          </button>
        ))}
      </div>

      <div role="tabpanel">
        {tab === "import" ? <SmartDataModelsImport onImport={onImport} /> : null}
        {tab === "structure" ? (
          <LinkmlVisualEditor
            source={source}
            onChange={setSource}
            diagnostics={diagnostics}
            locales={locales}
          />
        ) : null}
        {tab === "source" ? (
          <LinkmlSourceEditor source={source} onChange={setSource} diagnostics={diagnostics} />
        ) : null}
        {tab === "preview" ? <LinkmlPreviewPanel source={source} /> : null}
        {tab === "mappings" ? (
          <MappingsEditor
            models={[
              { name: model.name ?? "draft", version: nextVersion, source },
              ...mappable.filter((candidate) => candidate.name !== model.name),
            ]}
          />
        ) : null}
      </div>
    </div>
  );
}
