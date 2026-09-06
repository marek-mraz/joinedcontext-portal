import { useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { isChange, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { blueprintSpec } from "./Gallery";

type Parameters = Record<string, unknown>;

/**
 * The instantiation wizard: one form, generated from the blueprint's own parameter schema
 * (CC-24, CC-31), never hand-built per blueprint. Saving proposes a change like every other
 * write in the Portal, so the user never sees a branch or a merge request (CC-29, CC-32).
 *
 * The same schema validates in the browser and on the server. The browser copy is what makes
 * the form usable; the server copy is the one that decides (CC-60).
 */
export function Instantiate({
  project,
  blueprint,
  onBack,
}: {
  project: string;
  blueprint: Manifest;
  onBack: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);

  const spec = blueprintSpec(blueprint);
  const name = localized(blueprint.metadata.title, i18n.language, blueprint.metadata.name);
  const schema = spec.parameterSchema as JsonSchema | undefined;

  const start = useMutation({
    mutationFn: async (parameters: Parameters) => {
      setError(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/flows", {
          params: { path: { project } },
          body: {
            blueprint: blueprint.metadata.name,
            // The version the form was generated from: expanding these values against a
            // schema the user never saw is how a form produces a manifest nobody reviewed.
            version: spec.version ?? "",
            parameters,
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
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("flows.instantiate.failed"),
      );
    },
  });

  return (
    <div className="space-y-4">
      <button
        type="button"
        onClick={onBack}
        className="rounded border border-border px-3 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
      >
        {t("flows.back")}
      </button>

      <h1 className="text-xl font-bold">{t("flows.instantiate.title", { name })}</h1>
      <p className="text-sm text-muted">
        {localized(blueprint.metadata.description, i18n.language, "")}
      </p>

      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <p role="alert" className="text-danger">
          {error}
        </p>
      )}

      {schema ? (
        <>
          <SchemaForm<Parameters>
            schema={schema}
            disabled={start.isPending}
            submitLabel={t("flows.instantiate.submit")}
            onSubmit={(parameters) => {
              start.mutate(parameters);
            }}
          />
          <p className="text-sm text-muted">{t("flows.instantiate.hint")}</p>
        </>
      ) : (
        <p>{t("flows.instantiate.noSchema")}</p>
      )}
    </div>
  );
}
