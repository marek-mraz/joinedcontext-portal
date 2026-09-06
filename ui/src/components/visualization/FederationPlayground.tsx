import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";

/**
 * The federation playground: context spaces as nodes, ContextSourceRegistrations as edges,
 * notification traffic as marks travelling along them (UI-17, UI-28). Everything is computed
 * in the browser from the props below — the component never fetches, so the demo runs with no
 * broker, no gateway and no session behind it.
 */

export interface PlaygroundSpace {
  /** `metadata.name` of the ContextSpace. */
  name: string;
  /** The space other spaces are federated into; drawn in the middle. */
  hub?: boolean;
  /** Entities the space holds, shown on the node. */
  entities?: number;
}

export interface PlaygroundLink {
  /** Registering space: the one that forwards a query (`metadata.name`). */
  from: string;
  /** Registered space: the context source the query reaches. */
  to: string;
  /** `information.entities[].type` of the registration; absent = every type. */
  entityType?: string;
}

/**
 * A Helsinki-shaped mock in the shape of the two demo apps: one hub federating the
 * transport and air-quality spaces. Replaced by live manifests when the graph is embedded
 * in the project views (T-0307).
 */
export const DEMO_SPACES: PlaygroundSpace[] = [
  { name: "helsinki-hub", hub: true, entities: 0 },
  { name: "hsl-transport", entities: 30 },
  { name: "air-quality", entities: 12 },
  { name: "energy", entities: 8 },
  { name: "waste", entities: 21 },
];

export const DEMO_LINKS: PlaygroundLink[] = [
  { from: "helsinki-hub", to: "hsl-transport", entityType: "Vehicle" },
  { from: "helsinki-hub", to: "air-quality", entityType: "AirQualityObserved" },
  { from: "helsinki-hub", to: "energy" },
  { from: "helsinki-hub", to: "waste", entityType: "WasteContainer" },
];

/** Node placement in the 0…`size` square: the hub in the middle, the rest on a ring. */
export function layoutSpaces(
  spaces: PlaygroundSpace[],
  size = 100,
): Record<string, { x: number; y: number }> {
  const centre = size / 2;
  const ring = spaces.filter((space) => !space.hub);
  const radius = size * 0.34;
  const positions: Record<string, { x: number; y: number }> = {};
  for (const space of spaces) {
    if (space.hub) {
      positions[space.name] = { x: centre, y: centre };
    }
  }
  ring.forEach((space, index) => {
    // Start at the top and go clockwise, so the same input always draws the same picture.
    const angle = (index / Math.max(ring.length, 1)) * 2 * Math.PI - Math.PI / 2;
    positions[space.name] = {
      x: centre + radius * Math.cos(angle),
      y: centre + radius * Math.sin(angle),
    };
  });
  return positions;
}

interface TravellingMark {
  id: number;
  linkIndex: number;
  /** Registered space → registering space: the answer travelling back to the caller. */
  entityType: string;
}

const VIEW = 100;
const NODE_RADIUS = 9;
const MARK_SECONDS = 1.6;
const LOG_LENGTH = 6;

function edgePath(
  a: { x: number; y: number },
  b: { x: number; y: number },
): { d: string; midX: number; midY: number } {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const distance = Math.hypot(dx, dy) || 1;
  // Trim both ends at the node border so the line touches the circles instead of hiding
  // under them, and bend it slightly, which keeps two registrations between one pair apart.
  const trim = NODE_RADIUS + 1.5;
  const start = { x: a.x + (dx / distance) * trim, y: a.y + (dy / distance) * trim };
  const end = { x: b.x - (dx / distance) * trim, y: b.y - (dy / distance) * trim };
  const midX = (start.x + end.x) / 2 + (-dy / distance) * 6;
  const midY = (start.y + end.y) / 2 + (dx / distance) * 6;
  return { d: `M ${start.x} ${start.y} Q ${midX} ${midY} ${end.x} ${end.y}`, midX, midY };
}

export interface FederationPlaygroundProps {
  spaces?: PlaygroundSpace[];
  links?: PlaygroundLink[];
  /** Milliseconds between two simulated notifications; the demo ticks about once a second. */
  intervalMs?: number;
}

export function FederationPlayground({
  spaces = DEMO_SPACES,
  links = DEMO_LINKS,
  intervalMs = 1200,
}: FederationPlaygroundProps): JSX.Element {
  const { t } = useTranslation();
  const [running, setRunning] = useState(true);
  // `marks` are in flight and disappear when they arrive; `log` is what the reader can
  // still read afterwards, which is also what a screen reader announces.
  const [marks, setMarks] = useState<TravellingMark[]>([]);
  const [log, setLog] = useState<TravellingMark[]>([]);
  const [selected, setSelected] = useState<string | undefined>();
  const nextId = useRef(0);
  const nextLink = useRef(0);

  const positions = useMemo(() => layoutSpaces(spaces, VIEW), [spaces]);
  const paths = useMemo(
    () =>
      links.map((link) => {
        const a = positions[link.from];
        const b = positions[link.to];
        return a && b ? edgePath(a, b) : undefined;
      }),
    [links, positions],
  );

  // The system setting wins over the animation: a mark still appears and is still logged,
  // it just does not fly across the screen (UI-04 accessibility baseline).
  const reducedMotion =
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-reduced-motion: reduce)").matches
      : false;

  const emit = useCallback(() => {
    if (links.length === 0) {
      return;
    }
    // Round robin, not random: the same demo shows the same sequence to every viewer.
    const linkIndex = nextLink.current % links.length;
    nextLink.current += 1;
    const link = links[linkIndex];
    const id = nextId.current;
    nextId.current += 1;
    const mark: TravellingMark = {
      id,
      linkIndex,
      entityType: link.entityType ?? t("playground.anyType"),
    };
    setMarks((current) => [...current, mark]);
    setLog((current) => [...current, mark].slice(-LOG_LENGTH));
    window.setTimeout(
      () => setMarks((current) => current.filter((other) => other.id !== id)),
      MARK_SECONDS * 1000,
    );
  }, [links, t]);

  useEffect(() => {
    if (!running) {
      return;
    }
    const timer = window.setInterval(emit, intervalMs);
    return () => window.clearInterval(timer);
  }, [emit, intervalMs, running]);

  const selectedLinks = selected
    ? links.filter((link) => link.from === selected || link.to === selected)
    : [];

  return (
    <section className="space-y-4" aria-labelledby="playground-title">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h2 id="playground-title" className="text-lg font-semibold">
            {t("playground.title")}
          </h2>
          <p className="text-sm text-surface-fg/70">{t("playground.subtitle")}</p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => setRunning((value) => !value)}
            aria-pressed={!running}
            className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {running ? t("playground.pause") : t("playground.resume")}
          </button>
          <button
            type="button"
            onClick={emit}
            className="rounded border border-border bg-surface px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
          >
            {t("playground.send")}
          </button>
        </div>
      </div>

      <svg
        role="img"
        aria-label={t("playground.graphLabel", { spaces: spaces.length, links: links.length })}
        viewBox={`0 0 ${VIEW} ${VIEW}`}
        className="h-[26rem] w-full rounded border border-border bg-surface-subtle"
      >
        {links.map((link, index) => {
          const path = paths[index];
          if (!path) {
            return null;
          }
          const active = marks.some((mark) => mark.linkIndex === index);
          return (
            <g key={`${link.from}-${link.to}-${link.entityType ?? "all"}`}>
              <path
                id={`playground-edge-${index}`}
                data-testid={`edge-${link.from}-${link.to}`}
                data-active={active ? "true" : "false"}
                d={path.d}
                fill="none"
                stroke={active ? "#7c3aed" : "#94a3b8"}
                strokeWidth={active ? 0.9 : 0.5}
              />
              <text
                x={path.midX}
                y={path.midY - 1.4}
                textAnchor="middle"
                className="fill-surface-fg/70"
                style={{ fontSize: "2.6px" }}
              >
                {link.entityType ?? t("playground.anyType")}
              </text>
            </g>
          );
        })}

        {marks.map((mark) => {
          const path = paths[mark.linkIndex];
          if (!path) {
            return null;
          }
          const link = links[mark.linkIndex];
          return (
            <circle
              key={mark.id}
              data-testid={`mark-${mark.linkIndex}`}
              r={1.8}
              fill="#7c3aed"
              cx={reducedMotion ? path.midX : undefined}
              cy={reducedMotion ? path.midY : undefined}
            >
              {reducedMotion ? null : (
                // The answer travels back from the context source to the space that asked,
                // which is why the motion is reversed along the same curve.
                <animateMotion
                  data-testid="mark-motion"
                  dur={`${MARK_SECONDS}s`}
                  repeatCount="1"
                  fill="freeze"
                  keyPoints="1;0"
                  keyTimes="0;1"
                  calcMode="linear"
                  path={path.d}
                />
              )}
              <title>{`${link.to} → ${link.from}`}</title>
            </circle>
          );
        })}

        {spaces.map((space) => {
          const position = positions[space.name];
          if (!position) {
            return null;
          }
          const isSelected = selected === space.name;
          return (
            <g
              key={space.name}
              role="button"
              tabIndex={0}
              aria-label={space.name}
              aria-pressed={isSelected}
              onClick={() => setSelected(isSelected ? undefined : space.name)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  setSelected(isSelected ? undefined : space.name);
                }
              }}
              className="cursor-pointer focus:outline-none"
            >
              <circle
                cx={position.x}
                cy={position.y}
                r={NODE_RADIUS}
                fill={space.hub ? "#1d4ed8" : "#0f766e"}
                stroke={isSelected ? "#f59e0b" : "#e2e8f0"}
                strokeWidth={isSelected ? 1.4 : 0.6}
              />
              <text
                x={position.x}
                y={position.y + 0.8}
                textAnchor="middle"
                fill="#ffffff"
                style={{ fontSize: "3px" }}
              >
                {space.name}
              </text>
              <text
                x={position.x}
                y={position.y + 4.4}
                textAnchor="middle"
                fill="#e2e8f0"
                style={{ fontSize: "2.6px" }}
              >
                {t("playground.entities", { count: space.entities ?? 0 })}
              </text>
            </g>
          );
        })}
      </svg>

      {selected ? (
        <div className="rounded border border-border p-3">
          <h3 className="text-sm font-semibold">
            {t("playground.selected", { space: selected })}
          </h3>
          <ul className="mt-1 space-y-0.5 text-sm">
            {selectedLinks.length === 0 ? (
              <li className="text-surface-fg/70">{t("playground.noLinks")}</li>
            ) : (
              selectedLinks.map((link) => (
                <li key={`${link.from}-${link.to}-${link.entityType ?? "all"}`}>
                  <span className="font-mono">{`${link.from} → ${link.to}`}</span>{" "}
                  <span className="text-surface-fg/70">
                    {link.entityType ?? t("playground.anyType")}
                  </span>
                </li>
              ))
            )}
          </ul>
        </div>
      ) : null}

      <div>
        <h3 className="text-sm font-semibold">{t("playground.events")}</h3>
        <ul aria-live="polite" className="mt-1 space-y-0.5 font-mono text-xs">
          {log.length === 0 ? (
            <li className="font-sans text-surface-fg/70">{t("playground.noEvents")}</li>
          ) : (
            [...log].reverse().map((mark) => {
              const link = links[mark.linkIndex];
              return (
                <li key={mark.id}>{`${link.to} → ${link.from} · ${mark.entityType}`}</li>
              );
            })
          )}
        </ul>
      </div>
    </section>
  );
}

export default FederationPlayground;
