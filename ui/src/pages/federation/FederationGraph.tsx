import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { edgePath, layoutSpaces } from "../../components/visualization/FederationPlayground";
import type { components } from "../../api/schema";

export type GraphNode = components["schemas"]["Node"];
export type GraphEdge = components["schemas"]["Edge"];
export type Graph = components["schemas"]["FederationGraph"];

const VIEW = 100;
const NODE_RADIUS = 6;

/** One colour per health, so the picture reads without the legend (UI-04). */
const HEALTH_FILL: Record<GraphNode["health"], string> = {
  ok: "var(--color-success, #15803d)",
  degraded: "var(--color-danger, #b91c1c)",
  unknown: "var(--color-border, #9ca3af)",
};

/**
 * Where each node sits. The spaces go in the middle and everything else on the ring around
 * them, which is the shape a hub actually has: the registrations, endpoints and pipelines all
 * point inward at the space they serve.
 *
 * The playground's own layout is reused rather than copied, so the two pictures of a
 * federation stay one picture with two data sources behind it.
 */
export function positionsOf(nodes: GraphNode[]): Record<string, { x: number; y: number }> {
  return layoutSpaces(
    nodes.map((node) => ({ name: node.id, hub: node.kind === "ContextSpace" })),
    VIEW,
  );
}

export interface FederationGraphProps {
  graph: Graph;
  /** The node whose card is open, by id. */
  selected?: string;
  onSelect: (id: string | undefined) => void;
}

/**
 * The federation of one project as a graph (UI-27).
 *
 * Everything drawn here comes from the answer: no node is inferred and no edge is added, so
 * the picture cannot claim a link the manifests do not have. What a node shows is its kind,
 * its name, its title and its health, and nothing else — a registration's card says that it
 * authenticates and how, never with what (PF-48, EP-71).
 */
export function FederationGraph({ graph, selected, onSelect }: FederationGraphProps): JSX.Element {
  const { t } = useTranslation();
  const positions = useMemo(() => positionsOf(graph.nodes), [graph.nodes]);

  if (graph.nodes.length === 0) {
    return <p className="text-sm text-surface-fg/70">{t("federation.graph.empty")}</p>;
  }

  return (
    <figure className="rounded border border-border bg-surface p-4">
      <figcaption className="mb-2 text-sm font-medium">{t("federation.graph.title")}</figcaption>
      <svg
        viewBox={`0 0 ${VIEW} ${VIEW}`}
        role="group"
        aria-label={t("federation.graph.title")}
        className="h-[26rem] w-full"
      >
        <g>
          {graph.edges.map((edge) => {
            const a = positions[edge.from];
            const b = positions[edge.to];
            if (!a || !b) {
              return null;
            }
            const { d } = edgePath(a, b);
            return (
              <path
                key={`${edge.from}->${edge.to}:${edge.kind}`}
                d={d}
                fill="none"
                stroke="currentColor"
                strokeWidth={0.5}
                strokeOpacity={0.45}
                strokeDasharray={edge.kind === "registers" ? undefined : "2 1.5"}
                aria-label={t(`federation.edge.${edge.kind}`)}
              />
            );
          })}
        </g>
        {graph.nodes.map((node) => {
          const at = positions[node.id];
          if (!at) {
            return null;
          }
          const active = node.id === selected;
          return (
            <g key={node.id}>
              <circle
                cx={at.x}
                cy={at.y}
                r={NODE_RADIUS}
                fill={HEALTH_FILL[node.health]}
                stroke="currentColor"
                strokeWidth={active ? 1.2 : 0.4}
                role="button"
                tabIndex={0}
                aria-pressed={active}
                aria-label={`${t(`federation.kind.${node.kind}`)}: ${node.name}`}
                className="cursor-pointer"
                onClick={() => onSelect(active ? undefined : node.id)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    onSelect(active ? undefined : node.id);
                  }
                }}
              />
              <text
                x={at.x}
                y={at.y + NODE_RADIUS + 3.5}
                textAnchor="middle"
                fontSize={2.8}
                fill="currentColor"
              >
                {node.name}
              </text>
            </g>
          );
        })}
      </svg>
    </figure>
  );
}
