import { useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, readCsrfToken, unwrap } from "../../api/client";
import type { ProblemDetails } from "../../api/client";
import { asManifests, refName } from "../../api/manifest";
import { useOrgDomain } from "../../api/projects";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { Alert, Button, PageHeader } from "../../components/ui";
import { takePrefill } from "../../assistant/state";
import { getDraft, putDraft } from "../../api/drafts";
import { LinkmlEditor } from "./LinkmlEditor";
import { MappingsEditor } from "./MappingsEditor";
import type { MappingModel } from "./MappingsEditor";
import { ModelFileDrop } from "./ModelFileDrop";
import { mergeModels } from "./linkml";
import { SmartDataModelsImport } from "./SmartDataModelsImport";
import type { CatalogueModel } from "./SmartDataModelsImport";
import { blankSource, parseModel } from "./linkml";
import { applyOperations } from "./operations";
import type { Operation } from "./operations";
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

/** How long a keystroke waits before the draft is kept, as the resource form waits (UI-47). */
const DRAFT_DEBOUNCE_MS = 600;

/**
 * A free-typed or LinkML name as a manifest name: lower-case letters, digits and `-` (DM-57).
 * `undefined` when nothing is left, which is what keeps Save out of reach for an unnamed draft.
 */
function manifestName(name: string): string | undefined {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug === "" ? undefined : slug;
}

export function ModelsPage({
  project,
  locales = ["sk", "en", "de", "cs"],
  baseline,
  mappable = [],
}: ModelsPageProps): JSX.Element {
  const { t } = useTranslation();
  // A new model's IRIs are minted under the organization's own domain (DM-13), never a guess.
  const orgDomain = useOrgDomain(project);
  // What the assistant's dock left for this page: a draft from a dropped file opens straight in
  // the editor, and a model it changed (`?edit=<name>`, AG-77) opens with its operations, which
  // are applied to the source once it is loaded (DM-13).
  const [handedOff] = useState(() => {
    const prefill = takePrefill(window.location.pathname) as { source?: unknown; operations?: unknown } | null;
    const params = new URLSearchParams(window.location.search);
    return {
      edit: params.get("edit") ?? undefined,
      // The draft this page was holding when it was last open (DM-57, T-1029).
      draft: params.get("draft") ?? undefined,
      source: typeof prefill?.source === "string" ? prefill.source : undefined,
      operations: Array.isArray(prefill?.operations) ? (prefill.operations as Operation[]) : undefined,
    };
  });
  const prefilled = handedOff.source;
  const [editing, setEditing] = useState(baseline ? undefined : handedOff.edit);
  const [tabChoice, setTab] = useState<Tab | undefined>(
    baseline || prefilled || editing ? "editor" : undefined,
  );
  const [chosen, setChosen] = useState(baseline);
  const [breakingConfirmed, setBreakingConfirmed] = useState(false);
  /** What a second import could not take because the model already had it (T-1102). */
  const [importConflicts, setImportConflicts] = useState<string[]>([]);
  // What the person typed; before the first keystroke the source is the loaded or blank one.
  const [edited, setEdited] = useState<string | undefined>(baseline?.source ?? prefilled);
  const [saving, setSaving] = useState(false);
  const [checking, setChecking] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [checkInfo, setCheckInfo] = useState<{ severity: string; version: string } | null>(null);
  const [changeNotice, setChangeNotice] = useState<Change | null>(null);
  const [nameChoice, setNewName] = useState<string | undefined>(undefined);
  const [spaceChoice, setNewSpace] = useState<string | undefined>(undefined);
  /** The version of the shared draft this page last kept, for the next keep (AG-61). */
  const [kept, setKept] = useState<{ name: string; version: number } | null>(null);
  const [heldConflict, setHeldConflict] = useState(false);

  // A model imported or inferred here lives in this component until Save proposes it, and a
  // reload used to lose it. The Portal already shares drafts for exactly this (AG-61), so an
  // unpublished model is kept there under its own name and `?draft=` brings it back. What it
  // holds is a fallback, never an assignment: the first keystroke wins over it.
  const heldQuery = useQuery({
    queryKey: ["datamodel-draft", project, handedOff.draft],
    enabled: Boolean(handedOff.draft) && !baseline && prefilled === undefined,
    retry: false,
    queryFn: () => getDraft(project, "DataModel", handedOff.draft ?? ""),
  });
  const held = useMemo(() => {
    const draft = heldQuery.data;
    const spec = (draft?.manifest as { spec?: { linkml?: unknown; space?: unknown } } | undefined)?.spec;
    return draft && typeof spec?.linkml === "string"
      ? {
          name: draft.name,
          version: draft.version,
          source: spec.linkml,
          space: typeof spec.space === "string" ? spec.space : "",
        }
      : undefined;
  }, [heldQuery.data]);

  const newName = nameChoice ?? held?.name ?? "";
  const newSpace = spaceChoice ?? held?.space ?? "";
  const tab: Tab = tabChoice ?? (held ? "editor" : "import");

  // The list keys are shared with every page that lists these kinds, so the cache holds the list
  // as the API answers it and the manifests are read off it here (T-0625).
  const listOf = async (plural: string) =>
    unwrap(
      await api.GET("/api/v1/projects/{project}/{plural}", {
        params: { path: { project, plural } },
      }),
    );
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    retry: false,
    queryFn: () => listOf("datamodels"),
    select: (list) => asManifests(list.items ?? []),
  });
  const opened = useMemo((): ModelsPageProps["baseline"] => {
    const spec = models.data?.find((model) => model.metadata.name === editing)?.spec as
      | { version?: unknown; lifecycle?: unknown }
      | undefined;
    return editing && spec
      ? {
          name: editing,
          version: typeof spec.version === "string" ? spec.version : "0.1.0",
          lifecycle: (typeof spec.lifecycle === "string" ? spec.lifecycle : "draft") as Lifecycle,
        }
      : undefined;
  }, [models.data, editing]);
  const published = chosen ?? opened;

  const activeModelName = published?.name ?? baseline?.name ?? editing;
  // A draft nobody has published yet — inferred from a file, or started from blank — has no
  // manifest, so saving it creates one: it needs a name and the space it belongs to (DM-57).
  const creating = activeModelName === undefined;
  const targetName = activeModelName ?? manifestName(newName);
  const taken =
    creating && (models.data ?? []).some((manifest) => manifest.metadata.name === targetName);


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
  const handedOperations = editing === undefined ? undefined : handedOff.operations;
  const applied = useMemo(
    () =>
      handedOperations && loaded.data !== undefined ? applyOperations(loaded.data, handedOperations) : undefined,
    [handedOperations, loaded.data],
  );
  const source =
    edited ?? applied?.source ?? held?.source ?? loaded.data ?? blankSource(orgDomain, "new-model");
  const setSource = setEdited;
  const publishedSource = published?.source ?? loaded.data;

  const model = useMemo(() => parseModel(source), [source]);

  // What is kept: the manifest the Save would propose, minus the name the person may still
  // change. A model the project already publishes is not kept — its source lives in the
  // repository and the lane is how it changes — so this is the inferred and the imported one
  // (T-1029, T-1105), whichever tab produced it.
  const unpublished =
    models.isSuccess &&
    targetName !== undefined &&
    !(models.data ?? []).some((manifest) => manifest.metadata.name === targetName);
  const keeping =
    unpublished && source !== held?.source && edited !== undefined && !heldConflict;
  const keptVersion = [kept, held].find(
    (candidate) => candidate !== null && candidate !== undefined && candidate.name === targetName,
  )?.version;
  useEffect(() => {
    if (!keeping) {
      return;
    }
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const answer = await putDraft(
            project,
            "DataModel",
            targetName,
            {
              kind: "DataModel",
              metadata: { name: targetName },
              spec: { linkml: source, ...(newSpace ? { space: newSpace } : {}) },
            },
            keptVersion,
          );
          setKept({ name: targetName, version: answer.version });
          const url = new URL(window.location.href);
          if (url.searchParams.get("draft") !== targetName) {
            url.searchParams.set("draft", targetName);
            window.history.replaceState(null, "", url.toString());
          }
        } catch (err: unknown) {
          // Somebody else has this draft open and saved first; keeping ours would overwrite
          // theirs unseen, so the page stops keeping and says so (UI-48).
          if ((err as { status?: number }).status === 409) {
            setHeldConflict(true);
          }
        }
      })();
    }, DRAFT_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [keeping, project, targetName, source, newSpace, keptVersion]);

  // The project's other models, each with its source, so the Mappings tab has a pair to map
  // between (T-0795). A model the manifest carries inline is read from the manifest.
  const otherModels = (models.data ?? []).filter((manifest) => manifest.metadata.name !== activeModelName);
  const otherSources = useQueries({
    queries: otherModels.map((manifest) => ({
      queryKey: ["datamodel-source", project, manifest.metadata.name],
      retry: false,
      queryFn: async () => {
        const res = await fetch(
          `/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(manifest.metadata.name)}/source`,
          { credentials: "same-origin", headers: { Accept: "text/yaml, text/plain, */*" } },
        );
        if (!res.ok) {
          throw new Error(`HTTP ${res.status}`);
        }
        return res.text();
      },
    })),
  });
  const others: MappingModel[] = [
    ...mappable,
    ...otherModels.flatMap((manifest, index) => {
      const spec = manifest.spec as { version?: unknown; linkml?: unknown };
      const inline = typeof spec.linkml === "string" && spec.linkml.includes("\n") ? spec.linkml : undefined;
      const text = inline ?? otherSources[index]?.data;
      return text === undefined
        ? []
        : [
            {
              name: manifest.metadata.name,
              version: typeof spec.version === "string" ? spec.version : "1.0.0",
              source: text,
            },
          ];
    }),
  ];

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
  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    retry: false,
    queryFn: () => listOf("spaces"),
    select: (list) => asManifests(list.items ?? []),
  });
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    retry: false,
    queryFn: () => listOf("endpoints"),
    select: (list) => asManifests(list.items ?? []),
  });

  const consumers = useMemo(() => {
    const name = published?.name ?? model.name;
    if (!name) {
      return [];
    }
    const servingSpaces = (spaces.data ?? [])
      .filter((space) => refName((space.spec as { dataModelRef?: unknown })?.dataModelRef) === name)
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

  // T-1102: a second import joins the model being edited instead of replacing it, so a Vehicle
  // and an AirQualityObserved can sit in one model and a slot can relate them. A name the model
  // already has is kept and reported: what is in hand may have been edited, and an import must
  // not undo that.
  const onImport = (imported: string, catalogueModel: CatalogueModel, space?: string) => {
    const editing = source.trim() !== "" && parseModel(source).classes.length > 0;
    const { source: joined, conflicts } = editing
      ? mergeModels(source, imported)
      : { source: imported, conflicts: [] };
    setSource(joined);
    setImportConflicts(conflicts.map((conflict) => `${conflict.section}: ${conflict.name}`));
    // The space the person picked in the import travels with the model (DM-57, T-1108);
    // without one the editor asks for it, as it always did.
    if (space) {
      setNewSpace(space);
    }
    if (!editing) {
      setChosen({
        source: imported,
        version: "1.0.0",
        lifecycle: "draft",
        name: catalogueModel.name,
      });
    }
    setTab("editor");
  };

  // A model inferred from a file is a new draft: nothing published to compare against. Its
  // LinkML name is the obvious manifest name, and the person can change it before saving.
  const onPopulate = (draft: string) => {
    setSource(draft);
    setChosen(undefined);
    setEditing(undefined);
    setNewName(parseModel(draft).name ?? "");
    setTab("editor");
  };

  // The space travels with a create, and with nothing else: for a model that exists the route
  // reads the space off its manifest (DM-56).
  const sourceUrl = (dryRun: boolean): string => {
    const params = new URLSearchParams();
    if (dryRun) {
      params.set("dryRun", "All");
    }
    if (creating && newSpace) {
      params.set("space", newSpace);
    }
    const query = params.toString();
    const path = `/api/v1/projects/${encodeURIComponent(project)}/datamodels/${encodeURIComponent(targetName ?? "")}/source`;
    return query ? `${path}?${query}` : path;
  };

  const handleCheck = async () => {
    if (!targetName) return;
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
        sourceUrl(true),
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
    if (!targetName) return;
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
        sourceUrl(false),
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
      <PageHeader
        title={t("models.title")}
        aside={
          <p className="text-sm text-surface-fg/70">
            {t("models.version", { version: nextVersion })}
          </p>
        }
        actions={
          targetName && (!creating || newSpace) ? (
            <>
              <Button size="sm" onClick={handleCheck} disabled={checking || saving || taken}>
                {t("models.source.saveCheck")}
              </Button>
              {severity === "breaking" ? (
                <label className="flex items-center gap-1.5 text-sm">
                  <input
                    type="checkbox"
                    checked={breakingConfirmed}
                    onChange={(event) => setBreakingConfirmed(event.target.checked)}
                  />
                  {t("models.source.confirmBreaking", { version: nextVersion })}
                </label>
              ) : null}
              <Button
                size="sm"
                variant="primary"
                onClick={handleSave}
                disabled={
                  saving || checking || taken || (severity === "breaking" && !breakingConfirmed)
                }
              >
                {t("models.source.save")}
              </Button>
              {published ? (
                <DeleteResourceAction
                  target={{ project, kind: "DataModel", plural: "datamodels", name: published.name }}
                />
              ) : null}
            </>
          ) : null
        }
      />

      {loadingSource ? (
        <p role="status" className="text-sm text-surface-fg/70">
          {t("models.source.loading")}
        </p>
      ) : null}

      {changeNotice ? <ChangeNotice change={changeNotice} project={project} /> : null}

      {applied && applied.refused.length > 0 ? (
        <Alert role="alert" tone="danger">
          <p>{t("models.source.handOffRefused")}</p>
          <ul className="mt-1 list-disc pl-5">
            {applied.refused.map((refusal) => (
              <li key={refusal.index}>{refusal.reason}</li>
            ))}
          </ul>
        </Alert>
      ) : null}

      {saveError || loadError ? (
        <Alert role="alert" tone="danger">
          {saveError ?? loadError}
        </Alert>
      ) : null}

      {checkInfo ? (
        <Alert tone="info">
          <span className="font-medium">
            {t(`models.source.severity.${checkInfo.severity}`)} · {t("models.source.willBecome", { version: checkInfo.version })}
          </span>
        </Alert>
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
          <h2 id="models-changes" className="text-sm font-semibold">
            {t(`models.severity.${severity}`)} · {t(`lane.${lane}`)}
          </h2>
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
            <SmartDataModelsImport
              onImport={onImport}
              spaces={(spaces.data ?? []).map((space) => space.metadata.name)}
            />
          </div>
        ) : null}
        {tab === "editor" ? (
          <div className="flex flex-col gap-3">
            {importConflicts.length > 0 ? (
              <Alert tone="info" role="status">
                {t("models.sdm.mergedKept", { names: importConflicts.join(", ") })}
              </Alert>
            ) : null}
            {creating ? (
              <section
                aria-labelledby="models-new"
                className="flex flex-col gap-2 rounded border border-border p-3"
              >
                <h2 id="models-new" className="text-sm font-semibold">
                  {t("models.create.title")}
                </h2>
                <div className="flex flex-wrap items-end gap-3">
                  <label className="flex flex-col gap-1 text-sm">
                    {t("models.create.name")}
                    <input
                      className="rounded border border-border px-2 py-1"
                      value={newName}
                      onChange={(event) => setNewName(event.target.value)}
                    />
                  </label>
                  <label className="flex flex-col gap-1 text-sm">
                    {t("models.create.space")}
                    <select
                      className="rounded border border-border px-2 py-1"
                      value={newSpace}
                      onChange={(event) => setNewSpace(event.target.value)}
                    >
                      <option value="">{t("models.create.chooseSpace")}</option>
                      {(spaces.data ?? []).map((space) => (
                        <option key={space.metadata.name} value={space.metadata.name}>
                          {space.metadata.name}
                        </option>
                      ))}
                    </select>
                  </label>
                  {targetName ? (
                    <p className="text-sm text-surface-fg/70">
                      {t("models.create.file", { name: targetName })}
                    </p>
                  ) : null}
                </div>
                {taken ? (
                  <p role="alert" className="text-sm text-danger-fg">
                    {t("models.create.taken", { name: targetName })}
                  </p>
                ) : null}
                {heldConflict ? (
                  <p role="alert" className="text-sm text-danger-fg">
                    {t("models.create.heldConflict")}
                  </p>
                ) : kept ?? held ? (
                  <p role="status" className="text-sm text-surface-fg/70">
                    {t("models.create.held", { name: (kept ?? held)?.name })}
                  </p>
                ) : null}
              </section>
            ) : null}
            <LinkmlEditor source={source} onChange={setSource} locales={locales} />
          </div>
        ) : null}
        {tab === "mappings" ? (
          <MappingsEditor
            project={project}
            models={[
              { name: model.name ?? "draft", version: nextVersion, source },
              ...others.filter((candidate) => candidate.name !== model.name),
            ]}
            spaceOf={(name) =>
              (spaces.data ?? []).find(
                (space) => refName((space.spec as { dataModelRef?: unknown })?.dataModelRef) === name,
              )?.metadata.name
            }
            onProposed={setChangeNotice}
          />
        ) : null}
      </div>
    </div>
  );
}
