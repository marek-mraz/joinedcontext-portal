import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, readCsrfToken, unwrap } from "../../api/client";
import type { ProblemDetails } from "../../api/client";
import { asManifests, refName } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { takePrefill } from "../../assistant/state";
import { LinkmlEditor } from "./LinkmlEditor";
import { MappingsEditor } from "./MappingsEditor";
import type { MappingModel } from "./MappingsEditor";
import { ModelFileDrop } from "./ModelFileDrop";
import { SmartDataModelsImport } from "./SmartDataModelsImport";
import type { CatalogueModel } from "./SmartDataModelsImport";
import { blankSource, parseModel } from "./linkml";
import {
  bumpVersion,
  classifyChanges,
  laneOf,
  refusalToSave,
  severityOf,
} from "./breaking_detector";
import type { Lifecycle } from "./breaking_detector";

/**
 * The models page: import a model, edit it, map it (DM-07, DM-13, DM-17, DM-23, DM-33).
 *
 * The document lives here as text and nowhere else. The shared `LinkmlEditor` edits that one
 * string, the import wizard replaces it and the mappings tab reads it, so the views cannot hold
 * different models. Nothing on this page writes to the platform: saving a model is a repository
 * change, which is the ordinary lane flow (CC-32, CC-63; the contract is T-0576).
 */
export interface ModelsPageProps {
  project: string;
  /** The organisation's locales, for the language maps every title needs (DM-15). */
  locales?: string[];
  /**
   * The version this edit started from. The import wizard sets it; a published model's source
   * is loaded from the portal route when name is provided.
   */
  baseline?: { source?: string; version: string; lifecycle: Lifecycle; name: string };
  /**
   * The other models this one can be mapped to and from (DM-33). The document being edited is
   * always among them, so a mapping can be written before it is published.
   */
  mappable?: MappingModel[];
}

type Tab = "import" | "editor" | "mappings";

const TABS: Tab[] = ["import", "editor", "mappings"];

export function ModelsPage({
  project,
  locales = ["sk", "en", "de", "cs"],
  baseline,
  mappable = [],
}: ModelsPageProps): JSX.Element {
  const { t } = useTranslation();
  // A draft left by the assistant's dock (a dropped file) opens straight in the editor.
  const [prefilled] = useState(() => {
    const prefill = takePrefill(window.location.pathname) as { source?: unknown } | null;
    return typeof prefill?.source === "string" ? prefill.source : undefined;
  });
  const [tab, setTab] = useState<Tab>(baseline || prefilled ? "editor" : "import");
  const [published, setPublished] = useState(baseline);
  // What the person typed; before the first keystroke the source is the loaded or blank one.
  const [edited, setEdited] = useState<string | undefined>(baseline?.source ?? prefilled);
  const [saving, setSaving] = useState(false);
  const [checking, setChecking] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [checkInfo, setCheckInfo] = useState<{ severity: string; version: string } | null>(null);
  const [changeNotice, setChangeNotice] = useState<Change | null>(null);

  const activeModelName = published?.name ?? baseline?.name;
  // A published model's source lives in the repository and is read through DM-56's route.
  const loaded = useQuery({
    queryKey: ["datamodel-source", project, activeModelName],
    enabled: Boolean(activeModelName) && !baseline?.source,
    queryFn: async () => {
      const res = await fetch(
        `/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(activeModelName ?? "")}/source`,
        { credentials: "same-origin", headers: { Accept: "text/yaml, text/plain, */*" } },
      );
      if (!res.ok) {
        throw new Error(`HTTP ${res.status}`);
      }
      return res.text();
    },
  });
  const loadingSource = loaded.isLoading;
  const loadError = loaded.error instanceof Error ? loaded.error.message : null;
  const source = edited ?? loaded.data ?? blankSource(`${project}.sk`, "new-model");
  const setSource = setEdited;
  const publishedSource = published?.source ?? loaded.data;

  const model = useMemo(() => parseModel(source), [source]);

  const changes = useMemo(
    () => (publishedSource ? classifyChanges(parseModel(publishedSource), model) : []),
    [publishedSource, model],
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
    setTab("editor");
  };

  // A model inferred from a file is a new draft: nothing published to compare against.
  const onPopulate = (draft: string) => {
    setSource(draft);
    setPublished(undefined);
    setTab("editor");
  };

  const handleCheck = async () => {
    if (!activeModelName) return;
    setChecking(true);
    setSaveError(null);
    setCheckInfo(null);
    try {
      const headers: Record<string, string> = {
        "Content-Type": "text/yaml; charset=utf-8",
      };
      const csrf = readCsrfToken();
      if (csrf) {
        headers["x-csrf-token"] = csrf;
      }
      const res = await fetch(
        `/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(activeModelName)}/source?dryRun=All`,
        {
          method: "PUT",
          credentials: "same-origin",
          headers,
          body: source,
        },
      );
      if (res.status === 200) {
        const data = (await res.json()) as { severity: string; version: string };
        setCheckInfo({ severity: data.severity, version: data.version });
      } else {
        const problem = (await res.json().catch(() => ({}))) as ProblemDetails;
        const msg =
          problem.detail ??
          (problem.errors && problem.errors.length > 0
            ? problem.errors.join("; ")
            : t("models.source.refused", { reason: problem.title ?? `HTTP ${res.status}` }));
        setSaveError(msg);
      }
    } catch (err: unknown) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(false);
    }
  };

  const handleSave = async () => {
    if (!activeModelName) return;
    setSaving(true);
    setSaveError(null);
    try {
      const headers: Record<string, string> = {
        "Content-Type": "text/yaml; charset=utf-8",
      };
      const csrf = readCsrfToken();
      if (csrf) {
        headers["x-csrf-token"] = csrf;
      }
      const res = await fetch(
        `/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(activeModelName)}/source`,
        {
          method: "PUT",
          credentials: "same-origin",
          headers,
          body: source,
        },
      );
      if (res.status === 202) {
        const change = (await res.json()) as Change;
        setChangeNotice(change);
      } else {
        const problem = (await res.json().catch(() => ({}))) as ProblemDetails;
        const msg =
          problem.detail ??
          (problem.errors && problem.errors.length > 0
            ? problem.errors.join("; ")
            : t("models.source.refused", { reason: problem.title ?? `HTTP ${res.status}` }));
        setSaveError(msg);
      }
    } catch (err: unknown) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-xl font-semibold">{t("models.title")}</h2>
        <div className="flex items-center gap-3">
          {activeModelName ? (
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={handleCheck}
                disabled={checking || saving}
                className="rounded border border-border px-3 py-1 text-sm font-medium hover:bg-surface-subtle"
              >
                {t("models.source.saveCheck")}
              </button>
              <button
                type="button"
                onClick={handleSave}
                disabled={saving || checking}
                className="rounded bg-primary px-3 py-1 text-sm font-medium text-primary-fg hover:opacity-90"
              >
                {t("models.source.save")}
              </button>
            </div>
          ) : null}
          <p className="text-sm text-surface-fg/70">
            {t("models.version", { version: nextVersion })}
          </p>
        </div>
      </header>

      {loadingSource ? (
        <p role="status" className="text-sm text-surface-fg/70">
          {t("models.source.loading")}
        </p>
      ) : null}

      {changeNotice ? <ChangeNotice change={changeNotice} project={project} /> : null}

      {saveError || loadError ? (
        <div role="alert" className="rounded border border-danger bg-danger/10 p-3 text-sm text-danger-fg">
          {saveError ?? loadError}
        </div>
      ) : null}

      {checkInfo ? (
        <div className="rounded border border-border bg-surface-subtle p-3 text-sm">
          <p className="font-medium">
            {t(`models.source.severity.${checkInfo.severity}`)} · {t("models.source.willBecome", { version: checkInfo.version })}
          </p>
        </div>
      ) : null}

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
        {tab === "import" ? (
          <div className="flex flex-col gap-4">
            <ModelFileDrop project={project} onPopulate={onPopulate} />
            <SmartDataModelsImport onImport={onImport} />
          </div>
        ) : null}
        {tab === "editor" ? (
          <LinkmlEditor source={source} onChange={setSource} locales={locales} />
        ) : null}
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
