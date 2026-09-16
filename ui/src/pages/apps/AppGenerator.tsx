import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { fetchJson, publishedTypes } from "../endpoints/SchemaProjectionPanel";
import type { PublishedType } from "../endpoints/SchemaProjectionPanel";
import { EndpointPreview, accessWords } from "./EndpointPreview";
import { useAccess } from "../../components/entities/AccessPanel";
import { Button, Input, PageHeader, Select, Textarea } from "../../components/ui";

/** The blueprint that turns a description into an app (AP-22, Architecture/16 §3). */
export const BLUEPRINT = "app-from-prompt";

/**
 * `spec.kind` of the App the blueprint writes. `static` is the kit pass, a dashboard inside a
 * minute from one model call (AP-56); `fullstack` is what the workspace builds (AP-25).
 */
export const APP_KINDS = ["static", "fullstack", "service"] as const;
export type AppKind = (typeof APP_KINDS)[number];

/** The two apps that ship with the platform, for a deployment with no builder to point at. */
export const EXAMPLE_APPS = ["hsl-transport", "air-quality"] as const;

/** How many endpoints one application may read (AP-44), the first one the primary. */
export const MAX_ENDPOINTS = 5;

/** The published model of one endpoint, read with the person's own session. */
export async function endpointSchema(slug: string): Promise<unknown> {
  const base = `${window.location.origin}/api/endpoint/${slug}/schema`;
  const index = (await fetchJson(`${base}/index.json`)) as { models?: { version?: number }[] };
  return fetchJson(`${base}/v${index.models?.[0]?.version ?? 1}/json-schema`);
}

/**
 * The request's endpoints (AP-44): `endpointName` for one, `endpointNames` with the primary first
 * when the person added more.
 */
export function endpointFields(primary: string, extra: string[]): Record<string, unknown> {
  const others = extra.filter((name) => name !== "" && name !== primary);
  return others.length === 0 ? { endpointName: primary } : { endpointNames: [primary, ...others] };
}

/** A generated app reads. It updates only when the person ticks it and their own grant allows it. */
const OPERATIONS = ["queryEntity", "retrieveEntity"];
const WRITE_OPERATION = "updateAttrs";

/** Words that say what to do, not what the app is: a name made of them says nothing. */
const FILLER = new Set(
  (
    "create generate make build show give add want need please can could would like let " +
    "me us i we you a an the of for with and or on in at to from by that which this these new " +
    "my our your some all app apps application applications dashboard page site web " +
    "vytvor vygeneruj urob sprav ukaz pridaj chcem potrebujem prosim novu nove novy " +
    "vytvorit udelej ukaz chci potrebuji prosim " +
    "erstelle erzeuge mach zeige gib fuge ich will brauche bitte eine einen ein der die das " +
    "fur mit und von im"
  ).split(" "),
);

/**
 * A name for the app, from what the person asked for.
 *
 * It becomes the path the app is served at, so it is the lower-case, dash-joined form a URL
 * takes, made of the first three words that say what the app is ("Create a map of the bike
 * stations" is `map-bike-stations`, not `create-a-map-of`). With nothing left it is the
 * endpoint's app; a person who wants another name writes it under the details.
 */
export function slugOf(prompt: string, endpointName = ""): string {
  const words = prompt
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((word) => word !== "" && !FILLER.has(word))
    .slice(0, 3);
  const fallback = `${endpointName}-app`.replace(/^-+/, "");
  const slug = (words.length > 0 ? words.join("-") : fallback).slice(0, 40).replace(/-+$/, "");
  return slug === "" ? "app" : slug;
}

interface EndpointSpec {
  slug?: string;
  /** A `Ref`: a bare name, or `{ kind, name }`. Read it with `refName`, never as a string. */
  contextSpaceRef?: unknown;
  audience?: string;
  enabledRepresentations?: string[];
}

export function endpointSpec(endpoint: Manifest): EndpointSpec {
  return endpoint.spec as EndpointSpec;
}

/**
 * What the app would be allowed to read, derived from what the Endpoint actually publishes.
 *
 * The types and attributes come from the endpoint's own schema surface, which is already the
 * projection its Policy allows, so nothing here can name an attribute the endpoint hides. The
 * user unticks what the app does not need; there is no control that adds one back, which is
 * what AP-22 means by the confirmed list being the grant rather than the prompt.
 */
/**
 * The types an app can read: the schema's definitions minus the base classes a model derives
 * from, which no entity is served as (`Entity`, or a definition the schema marks abstract).
 */
export function concreteTypes(document: unknown): ReturnType<typeof publishedTypes> {
  const defs = (document as { $defs?: Record<string, { abstract?: unknown }> } | null)?.$defs ?? {};
  return publishedTypes(document).filter(
    (type) => type.name !== "Entity" && defs[type.name]?.abstract !== true,
  );
}

export function dataNeeds(
  endpoint: Manifest,
  types: PublishedType[],
  dropped: string[],
  write = false,
): Record<string, unknown>[] {
  const spec = endpointSpec(endpoint);
  const kept = types
    .map((type) => ({
      name: type.name,
      attributes: type.attributes.filter((attr) => !dropped.includes(`${type.name}.${attr}`)),
    }))
    .filter((type) => type.attributes.length > 0);
  if (kept.length === 0) {
    return [];
  }
  // Exactly the fields of jc-core's `DataNeed`: this value becomes `spec.dataNeeds` of the App
  // manifest the run publishes, and that kind refuses a field it does not know.
  return [
    {
      contextSpaceRef: { kind: "ContextSpace", name: refName(spec.contextSpaceRef) },
      types: kept.map((type) => type.name),
      attrs: [...new Set(kept.flatMap((type) => type.attributes))].sort(),
      operations: write ? [...OPERATIONS, WRITE_OPERATION] : OPERATIONS,
      representations: spec.enabledRepresentations ?? [],
    },
  ];
}

/**
 * "Generate your own app" (AP-22, AP-30, AP-51, AG-26, AG-43).
 *
 * The form is the whole of what a person has to decide: which endpoint, what the app should do,
 * and which of the attributes the endpoint publishes it may read. Submitting it starts an agent
 * run (`POST …/agent-runs`), and the answer is a run to watch rather than a merge request to
 * wait for: the review comes at the end, when the person publishes what was built (AP-55).
 *
 * A deployment without the Agent Runner has no `app-from-prompt` blueprint to offer, which is
 * what the banner below says rather than a button that leads nowhere (ADR-N-014).
 */
export function AppGenerator({
  project,
  initialName,
  onStarted,
}: {
  project: string;
  initialName?: string;
  /** Called with the new run before the page moves to the app, so the assistant can follow it. */
  onStarted?: (runId: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState(initialName ?? "");
  const [kind, setKind] = useState<AppKind>("static");
  const [prompt, setPrompt] = useState("");
  const [endpointName, setEndpointName] = useState("");
  /** Endpoints read beside the primary one, e.g. an indicator space's (AP-44). */
  const [extra, setExtra] = useState<string[]>([]);
  const [addingEndpoint, setAddingEndpoint] = useState(false);
  const [dropped, setDropped] = useState<string[]>([]);
  const [write, setWrite] = useState(false);
  const [conflictApp, setConflictApp] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** One entry per rule the parameters broke, so every bad field is named at once (CC-24). */
  const [violations, setViolations] = useState<string[]>([]);

  const blueprints = useQuery({
    queryKey: ["blueprints"],
    queryFn: async () => unwrap(await api.GET("/api/v1/blueprints", {})),
  });

  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const available = asManifests(blueprints.data?.items ?? []).find(
    (blueprint) => blueprint.metadata.name === BLUEPRINT,
  );
  const choices = asManifests(endpoints.data?.items ?? []);
  const endpoint = choices.find((candidate) => candidate.metadata.name === endpointName);
  const slug = endpoint ? (endpointSpec(endpoint).slug ?? "") : "";
  // The option to update exists only where the person's own grant on the endpoint has a write
  // (AP-22, AP-62): the gateway evaluates each save anyway; this keeps the form honest.
  const access = useAccess(slug === "" ? undefined : slug);
  const writes = accessWords(access.data).writes;

  // The endpoint's published model, which is where the app's bounds come from.
  const schema = useQuery({
    queryKey: ["generator-endpoint-schema", slug],
    enabled: slug !== "",
    retry: false,
    queryFn: async () => endpointSchema(slug),
  });

  // Every added endpoint is read whole: all its concrete types and attributes, never written.
  const extraEndpoints = extra
    .filter((name) => name !== endpointName)
    .map((name) => choices.find((candidate) => candidate.metadata.name === name))
    .filter((candidate): candidate is Manifest => candidate !== undefined);
  const extraSchemas = useQueries({
    queries: extraEndpoints.map((candidate) => {
      const extraSlug = endpointSpec(candidate).slug ?? "";
      return {
        queryKey: ["generator-endpoint-schema", extraSlug],
        enabled: extraSlug !== "",
        retry: false,
        queryFn: async () => endpointSchema(extraSlug),
      };
    }),
  });

  // Both are a pass over a handful of names; the React Compiler memoizes them, and a manual
  // useMemo here only tells it a dependency might be mutated when none of them is.
  const types = concreteTypes(schema.data);
  const needs = endpoint
    ? [
        ...dataNeeds(endpoint, types, dropped, write && writes.length > 0),
        ...extraEndpoints.flatMap((candidate, i) =>
          dataNeeds(candidate, concreteTypes(extraSchemas[i]?.data), []),
        ),
      ]
    : [];

  const generate = useMutation({
    mutationFn: async () => {
      setError(null);
      setViolations([]);
      setConflictApp(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project } },
          body: {
            appName: chosen,
            appClass: kind,
            ...endpointFields(endpointName, extraEndpoints.map((candidate) => candidate.metadata.name)),
            prompt,
            // The confirmed list, derived from the endpoint: the run is refused if it names
            // anything the endpoint does not publish, so the two cannot drift (AP-44).
            dataNeeds: needs,
          } as never,
        }),
      );
    },
    onSuccess: (result) => {
      const created = result as unknown as { id?: string; appName?: string };
      const targetName = created.appName || chosen;
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "agent-runs"] });
      if (typeof created.id === "string") {
        onStarted?.(created.id);
        void navigate({
          to: "/projects/$project/apps/$name",
          params: { project, name: targetName },
        });
        return;
      }
      if (isChange(result)) {
        setChange(result);
      }
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.status === 409) {
          setConflictApp(chosen);
        }
        setViolations(err.problem?.errors ?? []);
        setError(err.problem?.detail ?? err.message);
        return;
      }
      setError(t("app.error.generic"));
    },
  });

  if (blueprints.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (!available) {
    return <NoBuilder />;
  }

  // A name is needed for the URL the app is served at, not for the conversation: it is derived
  // from what the person asked for and stays editable under the details.
  const chosen = name.trim() === "" ? slugOf(prompt, endpointName) : name.trim();
  const ready = chosen !== "" && prompt.trim() !== "" && endpointName !== "" && needs.length > 0;

  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        generate.mutate();
      }}
    >
      <PageHeader title={t("apps.generate.title")} description={t("apps.generate.subtitle")} />

      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <div role="alert" className="text-danger">
          <p>{error}</p>
          {conflictApp && (
            <p className="mt-1 text-sm">
              <Link
                to="/projects/$project/apps/$name"
                params={{ project, name: conflictApp }}
                className="underline hover:no-underline"
              >
                {t("apps.drafts.conflict", { name: conflictApp })}
              </Link>
            </p>
          )}
        </div>
      )}
      {violations.length > 0 && (
        <ul role="alert" className="list-disc pl-5 text-sm text-danger">
          {violations.map((violation) => (
            <li key={violation}>{violation}</li>
          ))}
        </ul>
      )}

      <div>
        <label className="block text-sm font-medium" htmlFor="generator-endpoint">
          {t("apps.generate.endpoint")}
        </label>
        <Select
          id="generator-endpoint"
          value={endpointName}
          onChange={(event) => {
            setEndpointName(event.target.value);
            setExtra((current) => current.filter((name) => name !== event.target.value));
            setDropped([]);
          }}
          className="mt-1"
        >
          <option value="">{t("apps.generate.pickEndpoint")}</option>
          {choices.map((candidate) => (
            <option key={candidate.metadata.name} value={candidate.metadata.name}>
              {localized(candidate.metadata.title, i18n.language, candidate.metadata.name)}
            </option>
          ))}
        </Select>
        <p className="mt-1 text-xs text-muted">{t("apps.generate.endpointHint")}</p>
        {endpointName !== "" && choices.length > 1 && (
          <div className="mt-2">
            {!addingEndpoint && extra.length === 0 ? (
              <button
                type="button"
                onClick={() => setAddingEndpoint(true)}
                className="text-sm text-primary underline hover:no-underline"
              >
                {t("apps.generate.addEndpoint")}
              </button>
            ) : (
              <fieldset>
                <legend className="text-sm font-medium">{t("apps.generate.moreEndpoints")}</legend>
                <p className="text-xs text-muted">{t("apps.generate.moreEndpointsHint")}</p>
                <ul className="mt-1 grid gap-1 sm:grid-cols-2">
                  {choices
                    .filter((candidate) => candidate.metadata.name !== endpointName)
                    .map((candidate) => {
                      const candidateName = candidate.metadata.name;
                      const checked = extra.includes(candidateName);
                      return (
                        <li key={candidateName}>
                          <label className="flex items-center gap-2 text-sm">
                            <input
                              type="checkbox"
                              checked={checked}
                              disabled={!checked && extra.length + 1 >= MAX_ENDPOINTS}
                              onChange={(event) => {
                                setExtra((current) =>
                                  event.target.checked
                                    ? [...current, candidateName]
                                    : current.filter((name) => name !== candidateName),
                                );
                              }}
                            />
                            {localized(candidate.metadata.title, i18n.language, candidateName)}
                          </label>
                        </li>
                      );
                    })}
                </ul>
              </fieldset>
            )}
          </div>
        )}
      </div>

      {/* What the endpoint gives you, read with your own session, before you describe the app. */}
      {slug !== "" && <EndpointPreview slug={slug} />}

      {/*
        The description is the whole brief, and the first turn of a conversation rather than a
        specification: the agent builds from it, shows what it built, and is told what to change
        next on the run page.
      */}
      <div>
        <label className="block text-sm font-medium" htmlFor="generator-prompt">
          {t("apps.generate.prompt")}
        </label>
        <Textarea
          id="generator-prompt"
          rows={5}
          value={prompt}
          placeholder={t("apps.generate.promptPlaceholder")}
          onChange={(event) => {
            setPrompt(event.target.value);
          }}
          className="mt-1"
        />
        <p className="mt-1 text-xs text-muted">{t("apps.generate.promptHint")}</p>
      </div>

      <details className="rounded border border-border px-4 py-2">
        <summary className="cursor-pointer text-sm font-medium">
          {t("apps.generate.details", { name: chosen === "" ? "…" : chosen })}
        </summary>

        <div className="mt-3 space-y-4">
          <div>
            <label className="block text-sm font-medium" htmlFor="generator-name">
              {t("apps.generate.name")}
            </label>
            <Input
              id="generator-name"
              value={name}
              placeholder={slugOf(prompt, endpointName)}
              onChange={(event) => {
                setName(event.target.value);
              }}
              className="mt-1"
            />
          </div>

          <div>
            <label className="block text-sm font-medium" htmlFor="generator-kind">
              {t("apps.generate.kind")}
            </label>
            <Select
              id="generator-kind"
              value={kind}
              onChange={(event) => {
                setKind(event.target.value as AppKind);
              }}
              className="mt-1"
            >
              {APP_KINDS.map((value) => (
                <option key={value} value={value}>
                  {t(`apps.generate.kinds.${value}`)}
                </option>
              ))}
            </Select>
          </div>

          {endpointName !== "" && (
            <NeedsChecklist
              audience={endpoint ? (endpointSpec(endpoint).audience ?? "") : ""}
              types={types}
              dropped={dropped}
              writes={writes}
              write={write}
              onWrite={setWrite}
              state={schema.isPending ? "loading" : schema.isError ? "unavailable" : "ready"}
              onToggle={(attribute) => {
                setDropped((current) =>
                  current.includes(attribute)
                    ? current.filter((name) => name !== attribute)
                    : [...current, attribute],
                );
              }}
            />
          )}
        </div>
      </details>

      <Button
        type="submit"
        variant="primary"
        disabled={!ready || generate.isPending}
      >
        {t("apps.generate.submit")}
      </Button>
    </form>
  );
}

/** The derived needs, as the checklist AP-22 asks for. */
function NeedsChecklist({
  audience,
  types,
  dropped,
  writes,
  write,
  onWrite,
  state,
  onToggle,
}: {
  audience: string;
  types: PublishedType[];
  dropped: string[];
  writes: string[];
  write: boolean;
  onWrite: (write: boolean) => void;
  state: "loading" | "unavailable" | "ready";
  onToggle: (attribute: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="generator-needs" className="space-y-2 rounded border border-border p-4">
      <h2 id="generator-needs" className="text-base font-semibold">
        {t("apps.generate.needs.title")}
      </h2>
      <p className="text-sm text-muted">{t("apps.generate.needs.hint")}</p>
      {audience !== "" && (
        <p className="text-sm text-muted">{t("apps.generate.needs.audience", { audience })}</p>
      )}
      <p className="text-sm text-muted">{t("apps.generate.needs.loginOnly")}</p>
      {writes.length > 0 && (
        <label className="flex items-center gap-1.5 text-sm">
          <input
            type="checkbox"
            checked={write}
            onChange={(event) => {
              onWrite(event.target.checked);
            }}
          />
          {t("apps.generate.needs.write", { actions: writes.join(", ") })}
        </label>
      )}
      {state === "loading" && <p role="status">{t("apps.generate.needs.loading")}</p>}
      {state === "unavailable" && (
        <p role="alert" className="text-danger">
          {t("apps.generate.needs.unavailable")}
        </p>
      )}
      {state === "ready" && types.length === 0 && <p>{t("apps.generate.needs.none")}</p>}
      {types.map((type) => (
        <fieldset key={type.name} className="mt-2">
          <legend className="text-sm font-medium">{type.name}</legend>
          <div className="mt-1 flex flex-wrap gap-3">
            {type.attributes.map((attribute) => {
              const id = `${type.name}.${attribute}`;
              return (
                <label key={id} className="flex items-center gap-1.5 text-sm">
                  <input
                    type="checkbox"
                    checked={!dropped.includes(id)}
                    onChange={() => {
                      onToggle(id);
                    }}
                  />
                  {attribute}
                </label>
              );
            })}
          </div>
        </fieldset>
      ))}
    </section>
  );
}

/**
 * A deployment without the Agent Runner (ADR-N-014 makes it optional). Saying so beats a
 * button that opens a merge request nothing will ever pick up, and the reference apps are
 * the worked examples a reader wanted anyway.
 */
function NoBuilder(): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-3">
      <PageHeader title={t("apps.generate.title")} />
      <p role="note" className="rounded border border-border bg-surface-subtle p-3 text-sm">
        {t("apps.generate.noBuilder")}
      </p>
      <p className="text-sm">{t("apps.generate.examplesHint")}</p>
      <ul className="list-disc pl-5 text-sm">
        {EXAMPLE_APPS.map((app) => (
          <li key={app}>
            <a
              href={`/apps/${app}/`}
              className="text-primary underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {t(`apps.generate.examples.${app}`)}
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
