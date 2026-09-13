import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { fetchJson, publishedTypes } from "../endpoints/SchemaProjectionPanel";
import type { PublishedType } from "../endpoints/SchemaProjectionPanel";
import { AgentRunPage } from "./AgentRunPage";
import { EndpointPreview } from "./EndpointPreview";

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

/** A generated app reads; a write would need the red lane and a grant the endpoint has not got. */
const OPERATIONS = ["queryEntity", "retrieveEntity"];

/**
 * A name for the app, from what the person asked for.
 *
 * It becomes the path the app is served at, so it is the lower-case, dash-joined form a URL
 * takes; a person who wants another one writes it under the details.
 */
export function slugOf(prompt: string): string {
  return prompt
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .split("-")
    .slice(0, 4)
    .join("-");
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
export function dataNeeds(
  endpoint: Manifest,
  types: PublishedType[],
  dropped: string[],
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
      operations: OPERATIONS,
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
export function AppGenerator({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [name, setName] = useState("");
  const [kind, setKind] = useState<AppKind>("static");
  const [prompt, setPrompt] = useState("");
  const [endpointName, setEndpointName] = useState("");
  const [dropped, setDropped] = useState<string[]>([]);
  const [runId, setRunId] = useState<string | null>(null);
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

  // The endpoint's published model, which is where the app's bounds come from.
  const schema = useQuery({
    queryKey: ["generator-endpoint-schema", slug],
    enabled: slug !== "",
    retry: false,
    queryFn: async () => {
      const base = `${window.location.origin}/api/endpoint/${slug}/schema`;
      const index = (await fetchJson(`${base}/index.json`)) as {
        models?: { version?: number }[];
      };
      return fetchJson(`${base}/v${index.models?.[0]?.version ?? 1}/json-schema`);
    },
  });

  // Both are a pass over a handful of names; the React Compiler memoizes them, and a manual
  // useMemo here only tells it a dependency might be mutated when none of them is.
  const types = publishedTypes(schema.data);
  const needs = endpoint ? dataNeeds(endpoint, types, dropped) : [];

  const generate = useMutation({
    mutationFn: async () => {
      setError(null);
      setViolations([]);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project } },
          body: {
            appName: chosen,
            appClass: kind,
            endpointName,
            prompt,
            // The confirmed list, derived from the endpoint: the run is refused if it names
            // anything the endpoint does not publish, so the two cannot drift (AP-44).
            dataNeeds: needs,
          } as never,
        }),
      );
    },
    onSuccess: (result) => {
      const created = result as unknown as { id?: string };
      if (typeof created.id === "string") {
        setRunId(created.id);
        return;
      }
      if (isChange(result)) {
        setChange(result);
      }
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        setViolations(err.problem?.errors ?? []);
        setError(err.problem?.detail ?? err.message);
        return;
      }
      setError(t("app.error.generic"));
    },
  });

  if (runId !== null) {
    return (
      <AgentRunPage
        project={project}
        runId={runId}
        onClose={() => {
          setRunId(null);
        }}
      />
    );
  }

  if (blueprints.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (!available) {
    return <NoBuilder />;
  }

  // A name is needed for the URL the app is served at, not for the conversation: it is derived
  // from what the person asked for and stays editable under the details.
  const chosen = name.trim() === "" ? slugOf(prompt) : name.trim();
  const ready = chosen !== "" && prompt.trim() !== "" && endpointName !== "" && needs.length > 0;

  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        generate.mutate();
      }}
    >
      <div>
        <h1 className="text-xl font-bold">{t("apps.generate.title")}</h1>
        <p className="mt-1 text-sm text-muted">{t("apps.generate.subtitle")}</p>
      </div>

      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <p role="alert" className="text-danger">
          {error}
        </p>
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
        <select
          id="generator-endpoint"
          value={endpointName}
          onChange={(event) => {
            setEndpointName(event.target.value);
            setDropped([]);
          }}
          className="mt-1 block w-full rounded border border-border bg-surface px-3 py-1.5 text-base"
        >
          <option value="">{t("apps.generate.pickEndpoint")}</option>
          {choices.map((candidate) => (
            <option key={candidate.metadata.name} value={candidate.metadata.name}>
              {localized(candidate.metadata.title, i18n.language, candidate.metadata.name)}
            </option>
          ))}
        </select>
        <p className="mt-1 text-xs text-muted">{t("apps.generate.endpointHint")}</p>
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
        <textarea
          id="generator-prompt"
          rows={5}
          value={prompt}
          placeholder={t("apps.generate.promptPlaceholder")}
          onChange={(event) => {
            setPrompt(event.target.value);
          }}
          className="mt-1 block w-full rounded border border-border bg-surface px-3 py-2 text-base"
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
            <input
              id="generator-name"
              value={name}
              placeholder={slugOf(prompt)}
              onChange={(event) => {
                setName(event.target.value);
              }}
              className="mt-1 block w-full rounded border border-border bg-surface px-3 py-1.5 text-base"
            />
          </div>

          <div>
            <label className="block text-sm font-medium" htmlFor="generator-kind">
              {t("apps.generate.kind")}
            </label>
            <select
              id="generator-kind"
              value={kind}
              onChange={(event) => {
                setKind(event.target.value as AppKind);
              }}
              className="mt-1 block w-full rounded border border-border bg-surface px-3 py-1.5 text-base"
            >
              {APP_KINDS.map((value) => (
                <option key={value} value={value}>
                  {t(`apps.generate.kinds.${value}`)}
                </option>
              ))}
            </select>
          </div>

          {endpointName !== "" && (
            <NeedsChecklist
              audience={endpoint ? (endpointSpec(endpoint).audience ?? "") : ""}
              types={types}
              dropped={dropped}
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

      <button
        type="submit"
        disabled={!ready || generate.isPending}
        className="inline-flex items-center justify-center rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {t("apps.generate.submit")}
      </button>
    </form>
  );
}

/** The derived needs, as the checklist AP-22 asks for. */
function NeedsChecklist({
  audience,
  types,
  dropped,
  state,
  onToggle,
}: {
  audience: string;
  types: PublishedType[];
  dropped: string[];
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
      <h1 className="text-xl font-bold">{t("apps.generate.title")}</h1>
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
