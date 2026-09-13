import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { Manifest } from "../../api/manifest";
import { Button } from "../../components/ui";
import { COMPUTE_KINDS } from "../../schemas/kinds";
import type { PipelineForm } from "./PipelineEditor";
import { sourceKindOf } from "./PipelineStudio";
import type { Trace } from "./PipelineTest";

export interface FlowNode {
  id: "source" | "compute" | "output";
  kind: string;
  label: string;
  summary: string;
  present: boolean;
}

export interface FlowEdge {
  from: FlowNode["id"];
  to: FlowNode["id"];
}

export function toFlow(form: PipelineForm | undefined): { nodes: FlowNode[]; edges: FlowEdge[] } {
  const sk = sourceKindOf(form);
  const sourceRef =
    form?.source?.dataSourceRef ||
    (typeof form?.source?.endpointRef === "object"
      ? (form.source.endpointRef as { name?: string })?.name
      : form?.source?.endpointRef) ||
    "";
  const queryType = (form?.source?.query?.type as string | undefined) ?? "";
  const sourceSummary = [sourceRef, queryType].filter(Boolean).join(" · ");

  const sourceNode: FlowNode = {
    id: "source",
    kind: sk,
    label: "Source",
    summary: sourceSummary,
    present: true,
  };

  const targetName = form?.targetEndpoint ? form.targetEndpoint.split(":").pop() ?? "" : "";
  const outputSummary = [form?.output?.type, form?.output?.mode, targetName]
    .filter(Boolean)
    .join(" · ");

  const outputNode: FlowNode = {
    id: "output",
    kind: "output",
    label: "Output",
    summary: outputSummary,
    present: true,
  };

  const hasCompute = Boolean(form?.compute?.kind);
  if (hasCompute && form?.compute?.kind) {
    let computeSummary = "";
    if (form.compute.kind === "bloblang") {
      computeSummary = (form.compute.bloblang ?? "").trim().split("\n")[0] ?? "";
    } else if (form.compute.kind === "mapping") {
      computeSummary = form.compute.mappingRef ?? "";
    } else if (form.compute.kind === "wasm" || form.compute.kind === "container") {
      computeSummary = [form.compute.module, form.compute.function].filter(Boolean).join(".");
    }

    const computeNode: FlowNode = {
      id: "compute",
      kind: form.compute.kind,
      label: "Compute",
      summary: computeSummary,
      present: true,
    };

    return {
      nodes: [sourceNode, computeNode, outputNode],
      edges: [
        { from: "source", to: "compute" },
        { from: "compute", to: "output" },
      ],
    };
  }

  return {
    nodes: [sourceNode, outputNode],
    edges: [{ from: "source", to: "output" }],
  };
}

export function setComputeKind(form: PipelineForm | undefined, kind: string | null): PipelineForm {
  const base: PipelineForm = form ? { ...form } : { class: "auto" };
  if (!kind) {
    const rest: PipelineForm = { ...base };
    delete rest.compute;
    return rest;
  }
  const prev = base.compute;
  const compute: NonNullable<PipelineForm["compute"]> = { kind };
  if (kind === "bloblang") {
    if (prev?.bloblang) compute.bloblang = prev.bloblang;
  } else if (kind === "mapping") {
    if (prev?.mappingRef) compute.mappingRef = prev.mappingRef;
  } else if (kind === "wasm" || kind === "container") {
    if (prev?.module) compute.module = prev.module;
    if (prev?.function) compute.function = prev.function;
  }
  return { ...base, compute };
}

export interface NodePaint {
  eventsIn?: number;
  eventsOut?: number;
  error?: string;
  state: "ok" | "error" | "skipped" | "idle";
}

export function paintOf(trace: Trace | null, nodes: FlowNode[]): Record<FlowNode["id"], NodePaint> {
  const result: Record<FlowNode["id"], NodePaint> = {
    source: { state: "idle" },
    compute: { state: "idle" },
    output: { state: "idle" },
  };

  if (!trace) {
    return result;
  }

  let sourceError: string | undefined;
  let computeError: string | undefined;
  let outputError: string | undefined;

  for (const err of trace.errors ?? []) {
    const stage = (err.stage || "").toLowerCase();
    if (stage.includes("mapping") || stage.includes("compute") || stage.includes("bloblang")) {
      if (!computeError) computeError = err.message;
    } else if (stage.includes("input") || stage.includes("source")) {
      if (!sourceError) sourceError = err.message;
    } else if (stage.includes("validation") || stage.includes("output")) {
      if (!outputError) outputError = err.message;
    } else {
      if (!computeError) computeError = err.message;
    }
  }

  if (!outputError) {
    const failedVal = trace.validation?.find((v) => !v.ok);
    if (failedVal) {
      outputError =
        failedVal.problems && failedVal.problems.length > 0
          ? failedVal.problems[0]
          : "validation failed";
    }
  }

  const errorsByNode: Record<FlowNode["id"], string | undefined> = {
    source: sourceError,
    compute: computeError,
    output: outputError,
  };

  const eventsInByNode: Record<FlowNode["id"], number | undefined> = {
    source: trace.input?.events ?? 0,
    compute: trace.input?.events ?? 0,
    output: trace.mapping?.length ?? 0,
  };

  const eventsOutByNode: Record<FlowNode["id"], number | undefined> = {
    source: trace.input?.events ?? 0,
    compute: trace.mapping?.length ?? 0,
    output: (trace.validation ?? []).filter((v) => v.ok).length,
  };

  let errorEncountered = false;
  for (const node of nodes) {
    const err = errorsByNode[node.id];
    let state: NodePaint["state"] = "ok";
    if (errorEncountered) {
      state = "skipped";
    } else if (err) {
      state = "error";
      errorEncountered = true;
    }
    result[node.id] = {
      eventsIn: eventsInByNode[node.id],
      eventsOut: eventsOutByNode[node.id],
      error: err,
      state,
    };
  }

  return result;
}

export interface PipelineFlowProps {
  form: PipelineForm | undefined;
  onChange: (f: PipelineForm) => void;
  trace: Trace | null;
  selected: FlowNode["id"] | null;
  onSelect: (id: FlowNode["id"] | null) => void;
  dataSources: Manifest[];
  endpoints: Manifest[];
}

export function PipelineFlow({
  form,
  onChange,
  trace,
  selected,
  onSelect,
}: PipelineFlowProps): JSX.Element {
  const { t } = useTranslation();
  const { nodes, edges } = toFlow(form);
  const paint = paintOf(trace, nodes);

  const nodeWidth = 200;
  const nodeHeight = 100;
  const nodeSpacing = 240;
  const startX = 20;
  const startY = 25;
  const svgWidth = Math.max(nodes.length * nodeSpacing + 40, 520);
  const svgHeight = 150;

  return (
    <div className="flex flex-col gap-3">
      {/* Palette */}
      <div
        role="toolbar"
        aria-label={t("pipelines.flow.palette", { defaultValue: "Compute palette" })}
        className="flex flex-wrap items-center gap-2"
      >
        <span className="text-caption font-semibold text-fg">
          {t("pipelines.flow.palette", { defaultValue: "Palette" })}:
        </span>
        {COMPUTE_KINDS.map((kind) => (
          <Button
            key={kind}
            size="sm"
            variant="secondary"
            data-testid={`palette-${kind}`}
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData("text/plain", kind);
            }}
            onClick={() => {
              onChange(setComputeKind(form, kind));
              onSelect("compute");
            }}
          >
            + {kind}
          </Button>
        ))}
        {form?.compute?.kind ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              onChange(setComputeKind(form, null));
              if (selected === "compute") onSelect(null);
            }}
          >
            {t("pipelines.flow.remove", { defaultValue: "Remove compute" })}
          </Button>
        ) : null}
      </div>

      {/* SVG Canvas */}
      <div className="w-full overflow-x-auto rounded-md border border-border bg-surface-subtle p-2">
        <svg
          role="img"
          data-testid="flow-canvas"
          aria-label={t("pipelines.flow.canvas", { defaultValue: "Pipeline canvas" })}
          viewBox={`0 0 ${svgWidth} ${svgHeight}`}
          className="h-40 min-w-full"
          onDragOver={(e) => {
            e.preventDefault();
            e.dataTransfer.dropEffect = "copy";
          }}
          onDrop={(e) => {
            e.preventDefault();
            const kind = e.dataTransfer.getData("text/plain");
            if (COMPUTE_KINDS.includes(kind as (typeof COMPUTE_KINDS)[number])) {
              onChange(setComputeKind(form, kind));
              onSelect("compute");
            }
          }}
        >
          <defs>
            <marker
              id="flow-arrow"
              viewBox="0 0 10 10"
              refX="8"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto-start-reverse"
            >
              <path d="M 0 1.5 L 8 5 L 0 8.5 z" fill="#94a3b8" />
            </marker>
          </defs>

          {/* Edges */}
          {edges.map((edge) => {
            const fromIdx = nodes.findIndex((n) => n.id === edge.from);
            const toIdx = nodes.findIndex((n) => n.id === edge.to);
            if (fromIdx < 0 || toIdx < 0) return null;
            const x1 = startX + fromIdx * nodeSpacing + nodeWidth;
            const y1 = startY + nodeHeight / 2;
            const x2 = startX + toIdx * nodeSpacing;
            const y2 = startY + nodeHeight / 2;
            return (
              <line
                key={`${edge.from}-${edge.to}`}
                x1={x1}
                y1={y1}
                x2={x2 - 4}
                y2={y2}
                stroke="#94a3b8"
                strokeWidth={2}
                markerEnd="url(#flow-arrow)"
              />
            );
          })}

          {/* Nodes */}
          {nodes.map((node, idx) => {
            const x = startX + idx * nodeSpacing;
            const y = startY;
            const isSelected = selected === node.id;
            const nodePaint = paint[node.id] ?? { state: "idle" };
            const strokeColor =
              nodePaint.state === "error"
                ? "#dc2626"
                : nodePaint.state === "skipped"
                  ? "#9ca3af"
                  : nodePaint.state === "ok"
                    ? "#16a34a"
                    : isSelected
                      ? "#2563eb"
                      : "var(--color-border, #cbd5e1)";

            const summary =
              node.summary.length > 40 ? `${node.summary.slice(0, 39)}…` : node.summary;
            const errorSummary =
              nodePaint.error && nodePaint.error.length > 60
                ? `${nodePaint.error.slice(0, 59)}…`
                : nodePaint.error;

            return (
              <g
                key={node.id}
                role="button"
                tabIndex={0}
                data-testid={`flow-node-${node.id}`}
                data-state={nodePaint.state}
                aria-pressed={isSelected}
                aria-label={`${node.label}: ${node.kind}`}
                className="cursor-pointer focus:outline-none"
                onClick={() => onSelect(node.id)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onSelect(node.id);
                  } else if (
                    (e.key === "Delete" || e.key === "Backspace") &&
                    node.id === "compute"
                  ) {
                    e.preventDefault();
                    onChange(setComputeKind(form, null));
                    onSelect(null);
                  } else if (e.key === "ArrowRight") {
                    e.preventDefault();
                    const nextIdx = Math.min(idx + 1, nodes.length - 1);
                    onSelect(nodes[nextIdx].id);
                  } else if (e.key === "ArrowLeft") {
                    e.preventDefault();
                    const prevIdx = Math.max(idx - 1, 0);
                    onSelect(nodes[prevIdx].id);
                  }
                }}
              >
                <rect
                  x={x}
                  y={y}
                  width={nodeWidth}
                  height={nodeHeight}
                  rx={8}
                  fill="var(--color-surface, #ffffff)"
                  stroke={strokeColor}
                  strokeWidth={isSelected ? 2.5 : 1.5}
                />
                <text
                  x={x + 12}
                  y={y + 22}
                  className="font-semibold select-none"
                  style={{ fontSize: "13px" }}
                  fill="var(--color-fg, #0f172a)"
                >
                  {t(`pipelines.flow.node.${node.id}`, { defaultValue: node.label })}
                </text>
                <text
                  x={x + nodeWidth - 12}
                  y={y + 22}
                  textAnchor="end"
                  className="font-mono select-none"
                  style={{ fontSize: "11px" }}
                  fill="var(--color-fg-muted, #64748b)"
                >
                  {node.kind}
                </text>
                {summary ? (
                  <text
                    x={x + 12}
                    y={y + 44}
                    className="select-none"
                    style={{ fontSize: "11px" }}
                    fill="var(--color-fg-muted, #475569)"
                  >
                    {summary}
                  </text>
                ) : null}

                {nodePaint.eventsIn !== undefined ? (
                  <text
                    x={x + 12}
                    y={y + 66}
                    className="font-mono select-none"
                    style={{ fontSize: "11px" }}
                    fill="var(--color-fg-muted, #64748b)"
                  >
                    ↓ {nodePaint.eventsIn}  ↑ {nodePaint.eventsOut ?? 0}
                  </text>
                ) : null}

                {errorSummary ? (
                  <text
                    x={x + 12}
                    y={y + 86}
                    className="font-mono select-none"
                    style={{ fontSize: "10px" }}
                    fill="#dc2626"
                  >
                    {errorSummary}
                  </text>
                ) : nodePaint.state === "ok" ? (
                  <text
                    x={x + 12}
                    y={y + 86}
                    className="font-medium select-none"
                    style={{ fontSize: "11px" }}
                    fill="#16a34a"
                  >
                    ok
                  </text>
                ) : nodePaint.state === "skipped" ? (
                  <text
                    x={x + 12}
                    y={y + 86}
                    className="font-medium select-none"
                    style={{ fontSize: "11px" }}
                    fill="#9ca3af"
                  >
                    skipped
                  </text>
                ) : null}
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}
