import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { Instantiate } from "./Instantiate";

/** The three review lanes a blueprint declares (CC-59, CC-63). */
const RISK_STYLES: Record<string, string> = {
  green: "bg-emerald-500/15 border-emerald-500/40",
  yellow: "bg-amber-500/20 border-amber-500/40",
  red: "bg-danger/15 border-danger/40",
};

/** Sorts before every real category, so "Other" is the last chip rather than a random one. */
const UNCATEGORISED = "￿";

export interface BlueprintSpec {
  version?: string;
  category?: string | null;
  riskClass?: string;
  allowedRoles?: string[];
  parameterSchema?: Record<string, unknown>;
}

export function blueprintSpec(blueprint: Manifest): BlueprintSpec {
  return blueprint.spec as BlueprintSpec;
}

function RiskBadge({ riskClass }: { riskClass?: string }): JSX.Element | null {
  const { t } = useTranslation();
  const key = (riskClass ?? "").toLowerCase();
  if (!RISK_STYLES[key]) {
    return null;
  }
  return (
    <span
      className={`rounded border px-2 py-0.5 text-xs ${RISK_STYLES[key]}`}
      // The colour is never the only carrier: the label says what the lane costs the user.
      title={t("flows.riskLabel")}
    >
      {t(`flows.risk.${key}`)}
    </span>
  );
}

/**
 * The flow gallery: the primary way a user configures anything (CC-30). Cards, not YAML, and
 * only the blueprints this caller may run — the server filters by `spec.allowedRoles` (CC-59),
 * so a card that is missing here is refused there too.
 */
export function FlowGallery({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [category, setCategory] = useState<string | null>(null);
  const [selected, setSelected] = useState<Manifest | null>(null);

  const list = useQuery({
    queryKey: queryKeys.blueprints(),
    queryFn: async () => unwrap(await api.GET("/api/v1/blueprints", {})),
  });

  const blueprints = useMemo(() => asManifests(list.data?.items ?? []), [list.data]);

  const categories = useMemo(() => {
    const found = new Set<string>();
    for (const blueprint of blueprints) {
      found.add(blueprintSpec(blueprint).category || UNCATEGORISED);
    }
    return [...found].sort();
  }, [blueprints]);

  if (list.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  if (list.isError) {
    const message =
      list.error instanceof ApiError
        ? (list.error.problem?.detail ?? list.error.message)
        : t("app.error.generic");
    return (
      <div role="alert">
        <p className="text-danger">{message}</p>
        <button
          type="button"
          onClick={() => {
            void list.refetch();
          }}
          className="mt-2 rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {t("app.error.retry")}
        </button>
      </div>
    );
  }

  if (selected) {
    return (
      <Instantiate
        project={project}
        blueprint={selected}
        onBack={() => {
          setSelected(null);
        }}
      />
    );
  }

  const shown = category
    ? blueprints.filter((b) => (blueprintSpec(b).category || UNCATEGORISED) === category)
    : blueprints;

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold">{t("flows.title")}</h1>
        <p className="mt-1 text-sm text-muted">{t("flows.subtitle")}</p>
      </div>

      {categories.length > 1 && (
        <div className="flex flex-wrap gap-2" role="group" aria-label={t("flows.title")}>
          <FilterChip
            label={t("flows.category.all")}
            active={category === null}
            onClick={() => {
              setCategory(null);
            }}
          />
          {categories.map((name) => (
            <FilterChip
              key={name}
              label={name === UNCATEGORISED ? t("flows.category.uncategorised") : name}
              active={category === name}
              onClick={() => {
                setCategory(name);
              }}
            />
          ))}
        </div>
      )}

      {blueprints.length === 0 && <p>{t("flows.empty")}</p>}
      {blueprints.length > 0 && shown.length === 0 && <p>{t("flows.emptyFiltered")}</p>}

      <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {shown.map((blueprint) => {
          const spec = blueprintSpec(blueprint);
          const title = localized(blueprint.metadata.title, i18n.language, blueprint.metadata.name);
          return (
            <li
              key={blueprint.metadata.name}
              className="flex flex-col gap-2 rounded border border-border p-4"
            >
              <div className="flex items-start justify-between gap-2">
                <h2 className="font-semibold">{title}</h2>
                <RiskBadge riskClass={spec.riskClass} />
              </div>
              <p className="text-sm text-muted">
                {localized(blueprint.metadata.description, i18n.language, "")}
              </p>
              {spec.version ? (
                <p className="text-xs text-muted">{t("flows.version", { version: spec.version })}</p>
              ) : null}
              <button
                type="button"
                onClick={() => {
                  setSelected(blueprint);
                }}
                className="mt-auto self-start rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
              >
                {t("flows.run")}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function FilterChip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={`rounded-full border px-3 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-border-focus ${
        active ? "border-border-focus bg-surface-subtle" : "border-border hover:bg-surface-subtle"
      }`}
    >
      {label}
    </button>
  );
}
