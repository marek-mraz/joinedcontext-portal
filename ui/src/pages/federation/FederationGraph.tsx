import { useEffect, useMemo, useRef, useState } from "react";
import type { JSX, PointerEvent } from "react";
import { useTranslation } from "react-i18next";
import { edgePath, layoutSpaces } from "../../components/visualization/FederationPlayground";
import type { components } from "../../api/schema";

export type GraphNode = components["schemas"]["Node"];
export type GraphEdge = components["schemas"]["Edge"];
export type Graph = components["schemas"]["FederationGraph"];

const VIEW = 100;
const NODE_RADIUS = 6;
const MAX_SCALE = 4;
/** How far past the drawing a reader may pan, so a ring node can be brought to the middle. */
const PAN_MARGIN = 25;
/** A press that moves less than this many pixels is a click on a node, not a pan. */
const DRAG_THRESHOLD = 4;

/** The order the kind filters are offered in: the hub first, then what points at it. */
const KIND_ORDER = [
  "ContextSpace",
  "Endpoint",
  "ContextSourceRegistration",
  "ExternalSource",
  "Pipeline",
  "App",
  "CkanInstance",
];

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

/**
 * The graph without the kinds a reader switched off (UI-28). An edge goes with a hidden end;
 * an edge whose far end is missing from the answer altogether is left to the drawing, which
 * has nowhere to put it either way.
 */
export function withoutKinds(graph: Graph, hidden: ReadonlySet<string>): Graph {
  const gone = new Set(graph.nodes.filter((node) => hidden.has(node.kind)).map((node) => node.id));
  return {
    nodes: graph.nodes.filter((node) => !gone.has(node.id)),
    edges: graph.edges.filter((edge) => !gone.has(edge.from) && !gone.has(edge.to)),
  };
}

interface View {
  scale: number;
  x: number;
  y: number;
}

const HOME: View = { scale: 1, x: 0, y: 0 };

/** Keeps the window over the drawing, with a margin, whatever the zoom. */
function clamp(view: View): View {
  const scale = Math.min(MAX_SCALE, Math.max(1, view.scale));
  const size = VIEW / scale;
  const bound = (value: number) => Math.min(VIEW - size + PAN_MARGIN, Math.max(-PAN_MARGIN, value));
  return { scale, x: bound(view.x), y: bound(view.y) };
}

/** Zooms about the middle of what is on screen, so the node a reader looks at stays put. */
function zoomed(view: View, factor: number): View {
  const size = VIEW / view.scale;
  const scale = Math.min(MAX_SCALE, Math.max(1, view.scale * factor));
  const next = VIEW / scale;
  return clamp({ scale, x: view.x + (size - next) / 2, y: view.y + (size - next) / 2 });
}

export interface FederationGraphProps {
  graph: Graph;
  /** The node whose card is open, by id. */
  selected?: string;
  onSelect: (id: string | undefined) => void;
}

/**
 * The federation of one project as a graph (UI-27, UI-28).
 *
 * Everything drawn here comes from the answer: no node is inferred and no edge is added, so
 * the picture cannot claim a link the manifests do not have. What a node shows is its kind,
 * its name, its title and its health, and nothing else — a registration's card says that it
 * authenticates and how, never with what (PF-48, EP-71). The kind filters, the zoom and the pan
 * change the picture only.
 */
export function FederationGraph({ graph, selected, onSelect }: FederationGraphProps): JSX.Element {
  const { t } = useTranslation();
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());
  const [view, setView] = useState<View>(HOME);
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{ pointer: number; clientX: number; clientY: number; from: View; moved: boolean } | null>(
    null,
  );

  const kinds = useMemo(() => {
    const present = new Set(graph.nodes.map((node) => node.kind));
    return [...KIND_ORDER.filter((kind) => present.has(kind)), ...[...present].filter((kind) => !KIND_ORDER.includes(kind))];
  }, [graph.nodes]);
  const shown = useMemo(() => withoutKinds(graph, hidden), [graph, hidden]);
  const positions = useMemo(() => positionsOf(shown.nodes), [shown.nodes]);

  // A trackpad pinch arrives as a wheel event with ctrlKey. A plain wheel scrolls the page, so a
  // graph on a long page never traps the reader. React's own onWheel is passive and cannot stop
  // the browser zooming the whole page, hence the listener by hand.
  useEffect(() => {
    const element = svg.current;
    if (!element) {
      return;
    }
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey) {
        return;
      }
      event.preventDefault();
      setView((current) => zoomed(current, event.deltaY < 0 ? 1.25 : 0.8));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [graph.nodes.length]);

  if (graph.nodes.length === 0) {
    return <p className="text-sm text-surface-fg/70">{t("federation.graph.empty")}</p>;
  }

  const size = VIEW / view.scale;

  const toggle = (kind: string) => {
    const next = new Set(hidden);
    if (next.has(kind)) {
      next.delete(kind);
    } else {
      next.add(kind);
      if (graph.nodes.some((node) => node.id === selected && node.kind === kind)) {
        onSelect(undefined);
      }
    }
    setHidden(next);
  };

  const onPointerDown = (event: PointerEvent<SVGSVGElement>) => {
    drag.current = { pointer: event.pointerId, clientX: event.clientX, clientY: event.clientY, from: view, moved: false };
  };
  const onPointerMove = (event: PointerEvent<SVGSVGElement>) => {
    const start = drag.current;
    const width = svg.current?.clientWidth ?? 0;
    if (!start || start.pointer !== event.pointerId || width === 0) {
      return;
    }
    const dx = event.clientX - start.clientX;
    const dy = event.clientY - start.clientY;
    if (!start.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) {
      return;
    }
    if (!start.moved) {
      // Captured only once it is a pan: capturing on press would retarget the click of a node.
      start.moved = true;
      svg.current?.setPointerCapture?.(event.pointerId);
    }
    const perPixel = VIEW / start.from.scale / width;
    setView(clamp({ ...start.from, x: start.from.x - dx * perPixel, y: start.from.y - dy * perPixel }));
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  const control =
    "rounded border border-border px-2 py-0.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus disabled:opacity-50";

  return (
    <figure className="rounded border border-border bg-surface p-4">
      <figcaption className="mb-2 text-sm font-medium">{t("federation.graph.title")}</figcaption>
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <fieldset className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
          <legend className="sr-only">{t("federation.graph.filter")}</legend>
          {kinds.map((kind) => (
            <label key={kind} className="inline-flex items-center gap-1">
              <input type="checkbox" checked={!hidden.has(kind)} onChange={() => toggle(kind)} />
              {t(`federation.kind.${kind}`, { defaultValue: kind })}
            </label>
          ))}
        </fieldset>
        <div className="flex items-center gap-1">
          <button
            type="button"
            className={control}
            aria-label={t("federation.graph.zoomIn")}
            disabled={view.scale >= MAX_SCALE}
            onClick={() => setView((current) => zoomed(current, 1.5))}
          >
            +
          </button>
          <button
            type="button"
            className={control}
            aria-label={t("federation.graph.zoomOut")}
            disabled={view.scale <= 1}
            onClick={() => setView((current) => zoomed(current, 1 / 1.5))}
          >
            −
          </button>
          <button type="button" className={control} onClick={() => setView(HOME)}>
            {t("federation.graph.reset")}
          </button>
        </div>
      </div>
      <svg
        ref={svg}
        viewBox={`${view.x} ${view.y} ${size} ${size}`}
        role="group"
        aria-label={t("federation.graph.title")}
        className="h-[26rem] w-full cursor-grab touch-none select-none active:cursor-grabbing"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        <g>
          {shown.edges.map((edge) => {
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
                strokeDasharray={edge.kind === "registers" ? undefined : edge.kind === "publishes" ? "0.6 1.2" : "2 1.5"}
                aria-label={t(`federation.edge.${edge.kind}`)}
              />
            );
          })}
        </g>
        {shown.nodes.map((node) => {
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
                aria-label={`${t(`federation.kind.${node.kind}`, { defaultValue: node.kind })}: ${node.name}`}
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
