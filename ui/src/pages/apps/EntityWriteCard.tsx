import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { readCsrfToken } from "../../api/client";
import { Button } from "../../components/ui/Button";
import { localId, textOf } from "./QueryResultCard";

/**
 * A change to entities the assistant prepared (AG-78): every attribute before and after, and the
 * person's Apply, which sends each update from this browser with their own session through the
 * endpoint, so the gateway's policy decides it. Nothing is sent before Apply or after Cancel.
 */
export interface EntityWrite {
  endpoint: string;
  slug: string;
  entities: { id: string; changes: { attribute: string; before: unknown; after: unknown }[] }[];
}

type State =
  | { kind: "ready" }
  | { kind: "applying" }
  | { kind: "applied"; refused: { id: string; detail: string }[] }
  | { kind: "cancelled" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The preview of a `write_entities` step that passed its checks, or none. */
export function entityWriteOf(payload: Record<string, unknown>): EntityWrite | null {
  if (payload.tool !== "write_entities" || payload.status !== "ok" || !isRecord(payload.output)) {
    return null;
  }
  const { endpoint, slug, entities } = payload.output;
  if (typeof endpoint !== "string" || typeof slug !== "string" || !Array.isArray(entities)) {
    return null;
  }
  return {
    endpoint,
    slug,
    entities: entities.filter(isRecord).flatMap((entity) =>
      typeof entity.id === "string" && Array.isArray(entity.changes)
        ? [
            {
              id: entity.id,
              changes: entity.changes
                .filter(isRecord)
                .flatMap((change) =>
                  typeof change.attribute === "string"
                    ? [{ attribute: change.attribute, before: change.before, after: change.after }]
                    : [],
                ),
            },
          ]
        : [],
    ),
  };
}

/** Why the endpoint refused one update, in its own words when it gave any. */
async function refusalOf(response: Response, fallback: string): Promise<string> {
  const problem = (await response.json().catch(() => null)) as { detail?: unknown; title?: unknown } | null;
  return typeof problem?.detail === "string" && problem.detail !== ""
    ? problem.detail
    : typeof problem?.title === "string" && problem.title !== ""
      ? problem.title
      : fallback;
}

export function EntityWriteCard({ write, live }: { write: EntityWrite; live: boolean }): JSX.Element {
  const { t } = useTranslation();
  const [state, setState] = useState<State>({ kind: "ready" });

  const apply = async (): Promise<void> => {
    setState({ kind: "applying" });
    const refused: { id: string; detail: string }[] = [];
    for (const entity of write.entities) {
      try {
        const response = await fetch(
          `/api/endpoint/${encodeURIComponent(write.slug)}/ngsi-ld/v1/entities/${encodeURIComponent(entity.id)}/attrs`,
          {
            method: "PATCH",
            credentials: "same-origin",
            headers: { "content-type": "application/json", "x-csrf-token": readCsrfToken() ?? "" },
            body: JSON.stringify(
              Object.fromEntries(entity.changes.map((change) => [change.attribute, { type: "Property", value: change.after }])),
            ),
          },
        );
        if (!response.ok) {
          refused.push({
            id: entity.id,
            detail: await refusalOf(response, t("agentRun.write.refused", { status: response.status })),
          });
        }
      } catch {
        refused.push({ id: entity.id, detail: t("agentRun.write.unreachable") });
      }
    }
    setState({ kind: "applied", refused });
  };

  const changed = write.entities.length - (state.kind === "applied" ? state.refused.length : 0);
  return (
    <section
      aria-label={t("agentRun.write.title")}
      className="flex flex-col gap-2 rounded-lg border border-border bg-surface p-3 text-sm"
    >
      <p className="font-medium">
        {t("agentRun.write.lead", { count: write.entities.length, endpoint: write.endpoint })}
      </p>
      <div className="overflow-x-auto">
        <table className="min-w-full text-left text-xs">
          <thead className="text-fg-muted">
            <tr>
              <th className="py-0.5 pr-2 font-medium">{t("agentRun.write.entity")}</th>
              <th className="py-0.5 pr-2 font-medium">{t("agentRun.write.attribute")}</th>
              <th className="py-0.5 pr-2 font-medium">{t("agentRun.write.before")}</th>
              <th className="py-0.5 pr-2 font-medium">{t("agentRun.write.after")}</th>
            </tr>
          </thead>
          <tbody>
            {write.entities.flatMap((entity) =>
              entity.changes.map((change, index) => (
                <tr key={`${entity.id}-${change.attribute}`} className="border-t border-border">
                  <td className="py-0.5 pr-2" title={entity.id}>
                    {index === 0 ? localId(entity.id) : ""}
                  </td>
                  <td className="py-0.5 pr-2">{change.attribute}</td>
                  <td className="py-0.5 pr-2 text-fg-muted">{textOf(change.before) || t("agentRun.write.empty")}</td>
                  <td className="py-0.5 pr-2 font-medium">{textOf(change.after)}</td>
                </tr>
              )),
            )}
          </tbody>
        </table>
      </div>
      {state.kind === "applied" ? (
        <div role="status" className="flex flex-col gap-1">
          {changed > 0 ? <p>{t("agentRun.write.applied", { count: changed })}</p> : null}
          {state.refused.map((refusal) => (
            <p key={refusal.id} className="text-danger">
              {t("agentRun.write.refusedEntity", { entity: localId(refusal.id), reason: refusal.detail })}
            </p>
          ))}
        </div>
      ) : state.kind === "cancelled" ? (
        <p role="status" className="text-fg-muted">
          {t("agentRun.write.cancelled")}
        </p>
      ) : live ? (
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="primary" disabled={state.kind === "applying"} onClick={() => void apply()}>
            {t("agentRun.write.apply")}
          </Button>
          <Button size="sm" disabled={state.kind === "applying"} onClick={() => setState({ kind: "cancelled" })}>
            {t("agentRun.write.cancel")}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
