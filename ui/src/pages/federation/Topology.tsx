import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { localized } from "../../api/manifest";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { FederationGraph } from "./FederationGraph";
import type { GraphNode } from "./FederationGraph";

/** One key for every picture of a project's federation, so a proposal refreshes them all. */
export const federationGraphKey = (project: string) => ["projects", project, "federation-graph"] as const;

/** The list a node's kind is managed in, for the card's breadcrumb (UI-28). */
const LIST_OF: Record<string, "spaces" | "endpoints" | "pipelines" | "apps"> = {
  ContextSpace: "spaces",
  Endpoint: "endpoints",
  Pipeline: "pipelines",
  App: "apps",
};

/**
 * A project's federation graph with the card of the node a reader opens (UI-27, UI-28). The
 * federation page and the project's landing page draw the same component over the same query.
 */
export function Topology({ project }: { project: string }): JSX.Element | null {
  const [selected, setSelected] = useState<string | undefined>(undefined);
  const graph = useQuery({
    queryKey: federationGraphKey(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/federation-graph", {
          params: { path: { project } },
        }),
      ),
  });

  // Nothing drawn until a well-formed answer arrives: the landing page must not fail over a picture.
  if (!Array.isArray(graph.data?.nodes)) {
    return null;
  }
  const card = graph.data.nodes.find((node) => node.id === selected);

  return (
    <div className="space-y-4">
      <FederationGraph graph={graph.data} selected={selected} onSelect={setSelected} />
      {card ? <NodeCard project={project} node={card} /> : null}
    </div>
  );
}

function NodeCard({ project, node }: { project: string; node: GraphNode }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const kindLabel = t(`federation.kind.${node.kind}`, { defaultValue: node.kind });
  const plural = LIST_OF[node.kind];
  const link = "underline underline-offset-2 hover:text-primary";

  return (
    <aside aria-label={t("federation.card.title")} className="rounded border border-border p-4">
      <nav aria-label={t("federation.card.breadcrumb")} className="mb-2 text-caption text-surface-fg/70">
        <ol className="flex flex-wrap items-center gap-1">
          <li>{project}</li>
          <li aria-hidden="true">›</li>
          <li>
            {plural ? (
              <Link to="/projects/$project/$plural" params={{ project, plural }} className={link}>
                {t(`nav.${plural}`)}
              </Link>
            ) : node.kind === "CkanInstance" ? (
              <Link to="/projects/$project/ckan" params={{ project }} className={link}>
                {t("nav.ckan")}
              </Link>
            ) : (
              <Link to="/projects/$project/federation" params={{ project }} className={link}>
                {t("nav.federation")}
              </Link>
            )}
          </li>
          <li aria-hidden="true">›</li>
          <li aria-current="page" className="font-mono">
            {node.name}
          </li>
        </ol>
      </nav>
      <h2 className="text-sm font-semibold">
        {kindLabel}: <span className="font-mono">{node.name}</span>
      </h2>
      {Object.keys(node.title ?? {}).length > 0 ? (
        <p className="text-sm text-surface-fg/70">{localized(node.title, locale, node.name)}</p>
      ) : null}
      <dl className="mt-2 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-1 text-sm">
        {/* Health is read off the phase, so the chip says the same and more (Deploying, not
            just "not reported"); an object with no phase keeps the health line. */}
        {node.phase ? (
          <>
            <dt className="text-surface-fg/70">{t("federation.card.phase")}</dt>
            <dd>
              <LifecycleBadge kind="phase" value={node.phase} />
            </dd>
          </>
        ) : (
          <>
            <dt className="text-surface-fg/70">{t("federation.card.health")}</dt>
            <dd>{t(`federation.health.${node.health}`)}</dd>
          </>
        )}
        {node.registration ? (
          <>
            <dt className="text-surface-fg/70">{t("federation.field.mode")}</dt>
            <dd>{t(`federation.mode.${node.registration.mode}`)}</dd>
            <dt className="text-surface-fg/70">{t("federation.field.identity")}</dt>
            <dd>{t(`federation.identity.${node.registration.identity}`)}</dd>
            <dt className="text-surface-fg/70">{t("federation.field.entities")}</dt>
            <dd>{node.registration.types.join(", ")}</dd>
          </>
        ) : null}
      </dl>
      {/* The card says that a registration authenticates and how, never with what: the
          account is resolved when a query is forwarded, not drawn here (PF-48, UI-27). */}
      {node.registration ? (
        <p className="mt-2 text-xs text-surface-fg/70">{t("federation.card.noCredential")}</p>
      ) : null}
    </aside>
  );
}
