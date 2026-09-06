import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { fetchJson, publishedTypes } from "../endpoints/SchemaProjectionPanel";
import type { PublishedType } from "../endpoints/SchemaProjectionPanel";

/** The blueprint that turns a description into an app (AP-22, Architecture/16 §3). */
export const BLUEPRINT = "app-from-prompt";

/** `spec.kind` of the App the blueprint writes; `fullstack` is what it produces (AP-25). */
export const APP_KINDS = ["fullstack", "static", "service"] as const;
export type AppKind = (typeof APP_KINDS)[number];

/** The two apps that ship with the platform, for a deployment with no builder to point at. */
export const EXAMPLE_APPS = ["hsl-transport", "air-quality"] as const;

/** A generated app reads; a write would need the red lane and a grant the endpoint has not got. */
const OPERATIONS = ["queryEntity", "retrieveEntity"];

interface EndpointSpec {
  slug?: string;
  contextSpaceRef?: string;
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
  return [
    {
      contextSpaceRef: { kind: "ContextSpace", name: spec.contextSpaceRef ?? "" },
      types: kept.map((type) => type.name),
      attrs: [...new Set(kept.flatMap((type) => type.attributes))].sort(),
      operations: OPERATIONS,
      representations: spec.enabledRepresentations ?? [],
    },
  ];
}

/**
 * "Generate your own app" (AP-22, AP-30, AG-26).
 *
 * There is no builder API of its own: generation is the `app-from-prompt` blueprint started
 * through the ordinary flows route, so the answer is a merge request an approver sees like
 * any other change, and a deployment without the Agent Runner simply has no such blueprint to
 * offer. That is what the banner below says, rather than a button that leads nowhere.
 */
export function AppGenerator({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [name, setName] = useState("");
  const [kind, setKind] = useState<AppKind>("fullstack");
  const [prompt, setPrompt] = useState("");
  const [endpointName, setEndpointName] = useState("");
  const [dropped, setDropped] = useState<string[]>([]);
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
        await api.POST("/api/v1/projects/{project}/flows", {
          params: { path: { project } },
          body: {
            blueprint: BLUEPRINT,
            // The version the form was generated from; expanding these values against a
            // schema the user never saw is how a form produces a manifest nobody reviewed.
            version: (available?.spec as { version?: string } | undefined)?.version ?? "",
            parameters: { name, kind, prompt, endpoint: endpointName, dataNeeds: needs },
          },
        }),
      );
    },
    onSuccess: (result) => {
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

  if (blueprints.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (!available) {
    return <NoBuilder />;
  }

  const ready = name !== "" && prompt.trim() !== "" && endpointName !== "" && needs.length > 0;

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
        <label className="block text-sm font-medium" htmlFor="generator-name">
          {t("apps.generate.name")}
        </label>
        <input
          id="generator-name"
          value={name}
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

      <div>
        <label className="block text-sm font-medium" htmlFor="generator-prompt">
          {t("apps.generate.prompt")}
        </label>
        <textarea
          id="generator-prompt"
          rows={4}
          value={prompt}
          onChange={(event) => {
            setPrompt(event.target.value);
          }}
          className="mt-1 block w-full rounded border border-border bg-surface px-3 py-1.5 text-base"
        />
        <p className="mt-1 text-xs text-muted">{t("apps.generate.promptHint")}</p>
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
