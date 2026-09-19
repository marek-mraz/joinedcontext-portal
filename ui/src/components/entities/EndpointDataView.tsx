/**
 * What one Endpoint answers (T-1433; UI-69, EP-07): the grid, read only, over the endpoint itself.
 *
 * The types and their attributes come from the endpoint's own published schema rather than from the
 * DataModel, because the schema is already projected to the policy: what it lists is what this
 * endpoint may publish at all (EP-46, EP-61). An attribute the steward hides is left out of the
 * columns here too, so the effect of a tick in the form is visible as data and not only as a name.
 */
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parseGridConfig } from "@joinedcontext/sdk";
import { fetchJson, publishedTypes } from "../../pages/endpoints/SchemaProjectionPanel";
import { Field, Select } from "../ui";
import { PortalEntityGrid } from "./PortalEntityGrid";

interface SchemaIndex {
  models?: { name?: string; version?: number }[];
}

export interface EndpointDataViewProps {
  project: string;
  /** The endpoint's slug: the grid reads through it, never through a URL of its own (EP-55). */
  slug: string;
  /** `spec.projection.hiddenAttributes`, including what the open form has ticked but not saved. */
  hidden?: string[];
}

export function EndpointDataView({ project, slug, hidden = [] }: EndpointDataViewProps): JSX.Element {
  const { t } = useTranslation();
  const [chosen, setChosen] = useState<string>("");
  const base = `${window.location.origin}/api/endpoint/${slug}/schema`;

  const index = useQuery({
    queryKey: ["endpoint-schema-index", slug],
    retry: false,
    queryFn: () => fetchJson(`${base}/index.json`) as Promise<SchemaIndex>,
  });
  const version = index.data?.models?.[0]?.version ?? 1;
  const schema = useQuery({
    queryKey: ["endpoint-schema-artifact", slug, version, "json-schema"],
    enabled: index.isSuccess,
    retry: false,
    queryFn: () => fetchJson(`${base}/v${version}/json-schema`),
  });

  const types = useMemo(() => publishedTypes(schema.data), [schema.data]);
  const type = types.find((each) => each.name === chosen) ?? types[0];

  const config = useMemo(() => {
    if (!type) {
      return null;
    }
    const columns = type.attributes
      .filter((attribute) => !hidden.includes(attribute))
      .map((attribute) => ({ attr: attribute }));
    return (
      parseGridConfig({
        source: { kind: "endpoint", slug },
        type: type.name,
        columns,
        pageSize: 25,
        // Read only here: a value is corrected in the explorer, where the person chose the endpoint
        // for that purpose.
        mode: "view",
        history: { enabled: true },
      }).config ?? null
    );
  }, [slug, type, hidden]);

  if (index.isPending || schema.isPending) {
    return <p role="status">{t("endpoints.projection.loading")}</p>;
  }
  if (!type || !config) {
    return <p className="text-caption text-fg-muted">{t("endpoints.data.noSchema")}</p>;
  }

  return (
    <div className="flex flex-col gap-3">
      <Field id={`endpoint-data-type-${slug}`} label={t("endpoints.data.type")}>
        <Select
          id={`endpoint-data-type-${slug}`}
          value={type.name}
          onChange={(event) => setChosen(event.target.value)}
        >
          {types.map((each) => (
            <option key={each.name} value={each.name}>
              {each.name}
            </option>
          ))}
        </Select>
      </Field>
      <PortalEntityGrid key={`${slug}-${type.name}`} project={project} config={config} />
    </div>
  );
}
