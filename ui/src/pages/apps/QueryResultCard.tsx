import type { JSX } from "react";
import { useTranslation } from "react-i18next";

/**
 * What the assistant read from an endpoint through its data-plane MCP tools (AG-75): which
 * endpoint, which tool, the argument that matters, and the answer as a small table, a list of
 * fields or the text itself. The gateway wrote the answer and the model chose the call, so
 * every value is drawn as text, never as markup (AG-46).
 */

/** How many rows, attributes and characters the card draws; the step keeps the whole answer. */
export const MAX_ROWS = 10;
export const MAX_ATTRIBUTES = 5;
export const MAX_TEXT = 2000;

export type QueryView =
  | { kind: "table"; columns: string[]; rows: { id: string; cells: string[] }[]; total: number }
  | { kind: "fields"; fields: [string, string][] }
  | { kind: "text"; text: string };

export interface QueryResult {
  endpoint: string;
  tool: string;
  argument?: string;
  view: QueryView;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** An attribute's value as text: a normalized Property or Relationship unwrapped, keyValues as is. */
function unwrap(value: unknown): unknown {
  if (isRecord(value)) {
    if ("value" in value) {
      return value.value;
    }
    if ("object" in value) {
      return value.object;
    }
  }
  return value;
}

export function textOf(value: unknown): string {
  const plain = unwrap(value);
  if (plain === null || plain === undefined) {
    return "";
  }
  if (typeof plain === "string") {
    return plain;
  }
  if (typeof plain === "number" || typeof plain === "boolean") {
    return String(plain);
  }
  return JSON.stringify(plain);
}

/** The last segment of an NGSI-LD URN, which is what a person reads as the entity's name. */
export function localId(id: string): string {
  const parts = id.split(":");
  return parts[parts.length - 1] || id;
}

const SKIPPED = new Set(["id", "type", "@context"]);

function entitiesOf(value: unknown): Record<string, unknown>[] | null {
  const list = Array.isArray(value)
    ? value
    : isRecord(value) && Array.isArray(value.entities)
      ? value.entities
      : isRecord(value) && Array.isArray(value.items)
        ? value.items
        : null;
  if (list === null || !list.every((item) => isRecord(item) && typeof item.id === "string")) {
    return null;
  }
  return list as Record<string, unknown>[];
}

/** The answer of an MCP `tools/call`: its structured content, or the JSON its text holds, or the text. */
function answerOf(output: unknown): unknown {
  if (!isRecord(output)) {
    return output;
  }
  if (output.structuredContent !== undefined) {
    return output.structuredContent;
  }
  const content = Array.isArray(output.content) ? output.content : [];
  const first = content.find((part) => isRecord(part) && typeof part.text === "string") as
    | { text: string }
    | undefined;
  if (first === undefined) {
    return output;
  }
  try {
    return JSON.parse(first.text) as unknown;
  } catch {
    return first.text;
  }
}

export function viewOf(output: unknown): QueryView {
  const answer = answerOf(output);
  const entities = entitiesOf(answer);
  if (entities !== null) {
    const columns: string[] = [];
    for (const entity of entities) {
      for (const key of Object.keys(entity)) {
        if (!SKIPPED.has(key) && !columns.includes(key) && columns.length < MAX_ATTRIBUTES) {
          columns.push(key);
        }
      }
    }
    return {
      kind: "table",
      columns,
      rows: entities.slice(0, MAX_ROWS).map((entity) => ({
        id: localId(entity.id as string),
        cells: columns.map((column) => textOf(entity[column])),
      })),
      total: entities.length,
    };
  }
  if (isRecord(answer)) {
    return {
      kind: "fields",
      fields: Object.entries(answer)
        .filter(([key]) => key !== "@context")
        .map(([key, value]) => [key, textOf(value)]),
    };
  }
  const text = typeof answer === "string" ? answer : JSON.stringify(answer ?? "");
  return { kind: "text", text: text.slice(0, MAX_TEXT) };
}

/** The card of a `query_endpoint` step, or none when the payload is not one. */
export function queryResultOf(payload: Record<string, unknown>): QueryResult | null {
  if (payload.tool !== "query_endpoint" || !isRecord(payload.input)) {
    return null;
  }
  if (payload.status === "failed" || payload.error !== undefined || payload.output === undefined) {
    return null;
  }
  const input = payload.input;
  const endpoint = typeof input.endpoint === "string" ? input.endpoint : "";
  const tool = typeof input.name === "string" ? input.name : "";
  const args = isRecord(input.arguments) ? input.arguments : {};
  const key = ["type", "id", "q"].find((name) => typeof args[name] === "string" && args[name] !== "");
  return {
    endpoint,
    tool,
    argument: key === undefined ? undefined : `${key}: ${String(args[key])}`,
    view: viewOf(payload.output),
  };
}

export function QueryResultCard({ result }: { result: QueryResult }): JSX.Element {
  return (
    <div
      data-testid="query-result"
      className="rounded-lg border border-border bg-surface p-2 text-xs"
    >
      <p className="flex flex-wrap items-baseline gap-x-2 font-medium">
        <span>
          {result.endpoint} · {result.tool}
        </span>
        {result.argument !== undefined ? (
          <span className="font-mono text-fg-muted">{result.argument}</span>
        ) : null}
      </p>
      <QueryAnswer view={result.view} />
    </div>
  );
}

/** An answer as a small table of entities, a list of fields or the text itself. */
export function QueryAnswer({ view }: { view: QueryView }): JSX.Element {
  const { t } = useTranslation();
  return view.kind === "table" ? (
    <>
      <div className="mt-1 overflow-x-auto">
        <table className="min-w-full text-left">
          <thead className="text-fg-muted">
            <tr>
              <th className="py-0.5 pr-2 font-medium">{t("assistant.query.id")}</th>
              {view.columns.map((column) => (
                <th key={column} className="py-0.5 pr-2 font-medium">
                  {column}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {view.rows.map((row, index) => (
              <tr key={`${row.id}-${index}`} className="border-t border-border">
                <td className="py-0.5 pr-2 font-mono">{row.id}</td>
                {row.cells.map((cell, cellIndex) => (
                  <td key={view.columns[cellIndex]} className="max-w-[12rem] truncate py-0.5 pr-2" title={cell}>
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="mt-1 text-fg-muted">
        {view.total > view.rows.length
          ? t("assistant.query.someRows", { shown: view.rows.length, total: view.total })
          : t("assistant.query.rows", { count: view.total })}
      </p>
    </>
  ) : view.kind === "fields" ? (
    <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5">
      {view.fields.map(([key, value]) => (
        <div key={key} className="contents">
          <dt className="font-mono text-fg-muted">{key}</dt>
          <dd className="min-w-0 break-words">{value}</dd>
        </div>
      ))}
    </dl>
  ) : (
    <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono">{view.text}</pre>
  );
}
