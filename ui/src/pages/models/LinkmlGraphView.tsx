import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { graphData, parseModel } from "./linkml";
import type { GraphEdge, GraphNode } from "./linkml";

/** One box's size and the gaps around it, in the SVG's own units. */
const BOX = { width: 200, header: 26, row: 16, padding: 8, gapX: 40, gapY: 70 };
/** Past this many slots a box says "and n more" rather than growing down the page. */
const SLOTS_SHOWN = 6;

interface Placed extends GraphNode {
  x: number;
  y: number;
  height: number;
}

/**
 * Where each class sits: a row per `is_a` depth, in the model's own order within the row.
 *
 * No layout library. A class graph is a forest of short chains, so a parent above its children
 * and boxes left to right is the whole of it, and a dependency here would be one more thing to
 * pin, audit and ship for an SVG of a dozen rectangles.
 */
export function place(nodes: GraphNode[]): Placed[] {
  const rows = new Map<number, GraphNode[]>();
  for (const node of nodes) {
    rows.set(node.depth, [...(rows.get(node.depth) ?? []), node]);
  }
  const placed: Placed[] = [];
  for (const [depth, row] of [...rows.entries()].sort(([a], [b]) => a - b)) {
    row.forEach((node, index) => {
      const shown = Math.min(node.slots.length, SLOTS_SHOWN);
      const extra = node.slots.length > SLOTS_SHOWN ? 1 : 0;
      placed.push({
        ...node,
        x: index * (BOX.width + BOX.gapX),
        y: depth * (BOX.header + SLOTS_SHOWN * BOX.row + BOX.gapY),
        height: BOX.header + (shown + extra) * BOX.row + BOX.padding,
      });
    });
  }
  return placed;
}

/** The line between two boxes, from the bottom of one to the top of the other. */
function line(from: Placed, to: Placed): { x1: number; y1: number; x2: number; y2: number } {
  const middle = (box: Placed) => box.x + BOX.width / 2;
  const above = to.y + to.height <= from.y;
  return {
    x1: middle(from),
    y1: above ? from.y : from.y + from.height,
    x2: middle(to),
    y2: above ? to.y + to.height : to.y,
  };
}

/**
 * The model as a picture (DM-13, T-1111): every class a box with its own slots, and a line for
 * every way one class names another.
 *
 * Read-only, and deliberately so: a graph that also edited would be a third writer of the same
 * document beside the tree and the YAML, and the two that exist already share one string.
 * Clicking a class hands it to the caller, which opens it in the structure view.
 */
export function LinkmlGraphView({
  source,
  onOpenClass,
}: {
  source: string;
  onOpenClass?: (name: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { nodes, edges } = useMemo(() => graphData(parseModel(source)), [source]);
  const placed = useMemo(() => place(nodes), [nodes]);
  const at = useMemo(() => new Map(placed.map((node) => [node.name, node])), [placed]);

  if (placed.length === 0) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("models.graph.empty")}
      </p>
    );
  }

  const width = Math.max(...placed.map((node) => node.x + BOX.width)) + BOX.padding;
  const height = Math.max(...placed.map((node) => node.y + node.height)) + BOX.padding;
  const stroke = (edge: GraphEdge) =>
    edge.kind === "mixin" ? "6 4" : edge.kind === "range" ? "2 3" : undefined;

  return (
    <div className="flex flex-col gap-2">
      <p className="text-caption text-fg-muted">{t("models.graph.legend")}</p>
      <div className="overflow-auto rounded border border-border bg-surface p-2">
        <svg
          role="img"
          aria-label={t("models.graph.title")}
          viewBox={`-4 -4 ${width + 8} ${height + 8}`}
          className="min-w-full"
          style={{ minHeight: "12rem" }}
        >
          <defs>
            <marker
              id="linkml-arrow"
              viewBox="0 0 10 10"
              refX="9"
              refY="5"
              markerWidth="5"
              markerHeight="5"
              orient="auto-start-reverse"
            >
              <path d="M 0 1 L 9 5 L 0 9 z" fill="currentColor" />
            </marker>
          </defs>

          {edges.map((edge) => {
            const from = at.get(edge.from);
            const to = at.get(edge.to);
            if (!from || !to) {
              return null;
            }
            const { x1, y1, x2, y2 } = line(from, to);
            return (
              <g key={`${edge.kind}-${edge.from}-${edge.to}-${edge.label ?? ""}`} className="text-fg-subtle">
                <line
                  x1={x1}
                  y1={y1}
                  x2={x2}
                  y2={y2}
                  stroke="currentColor"
                  strokeWidth={1}
                  strokeDasharray={stroke(edge)}
                  markerEnd="url(#linkml-arrow)"
                />
                {edge.label ? (
                  <text
                    x={(x1 + x2) / 2}
                    y={(y1 + y2) / 2 - 2}
                    textAnchor="middle"
                    className="fill-current text-[9px]"
                  >
                    {edge.label}
                  </text>
                ) : null}
              </g>
            );
          })}

          {placed.map((node) => (
            <g
              key={node.name}
              role="button"
              tabIndex={0}
              aria-label={t("models.graph.openClass", { name: node.name })}
              className="cursor-pointer focus:outline-none"
              onClick={() => onOpenClass?.(node.name)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  onOpenClass?.(node.name);
                }
              }}
            >
              <rect
                x={node.x}
                y={node.y}
                width={BOX.width}
                height={node.height}
                rx={6}
                className="fill-surface-subtle stroke-border"
                strokeWidth={1}
              />
              <text x={node.x + BOX.padding} y={node.y + 17} className="fill-current text-[11px] font-semibold">
                {node.name}
              </text>
              {node.slots.slice(0, SLOTS_SHOWN).map((slot, index) => (
                <text
                  key={slot}
                  x={node.x + BOX.padding}
                  y={node.y + BOX.header + index * BOX.row + 4}
                  className="fill-current text-[10px] text-fg-muted"
                >
                  {slot}
                </text>
              ))}
              {node.slots.length > SLOTS_SHOWN ? (
                <text
                  x={node.x + BOX.padding}
                  y={node.y + BOX.header + SLOTS_SHOWN * BOX.row + 4}
                  className="fill-current text-[10px] text-fg-muted"
                >
                  {t("models.graph.more", { count: node.slots.length - SLOTS_SHOWN })}
                </text>
              ) : null}
            </g>
          ))}
        </svg>
      </div>
    </div>
  );
}
