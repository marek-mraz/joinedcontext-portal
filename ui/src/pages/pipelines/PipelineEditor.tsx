import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, prune } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useBranding } from "../../branding";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import type { ManifestSource } from "../../components/ResourceFormDialog";
import { Alert, buttonClass, Icon } from "../../components/ui";
import { pipelineSchema, pipelineUiSchema } from "../../schemas/kinds";
import type { EndpointOption } from "../../schemas/kinds";

/** The form of a pipeline: `PipelineSpec` with every reference flattened to its name. */
export interface PipelineForm {
  name?: string;
  title?: Record<string, string>;
  class?: string;
  schedule?: string;
  period?: string;
  source?: {
    dataSourceRef?: string;
    endpointRef?: string;
    query?: Record<string, unknown>;
    trigger?: { subscription?: { type?: string; watchedAttributes?: string[] } };
  };
  compute?: { kind?: string; module?: string; function?: string; mappingRef?: string };
  targetEndpoint?: string;
  output?: { type?: string; mode?: string };
  allowFeedback?: boolean;
  secretRefs?: { name?: string; key?: string; envVar?: string }[];
  quotas?: { maxMemoryMb?: number; cpuMillicores?: number };
}

const PLURAL = "pipelines";

/** A `Ref` is a bare name or `{ kind, name }` (MF-07); the form keeps the name. */
function refName(ref: unknown): string | undefined {
  if (typeof ref === "string") {
    return ref;
  }
  const typed = (ref ?? {}) as { name?: unknown };
  return typeof typed.name === "string" ? typed.name : undefined;
}

function typedRef(kind: string, name: string | undefined) {
  return name ? { kind, name } : undefined;
}

/**
 * The URN a pipeline writes through: `urn:ngsi-ld:Endpoint:{orgDomain}:{space}:{name}`
 * (PF-39, PL-18). The organization's domain comes from the branding; without it, or without
 * the endpoint's space, there is no URN to offer and the field takes one typed in full.
 */
export function endpointUrn(orgDomain: string, endpoint: Manifest): string | undefined {
  const space = (endpoint.spec as { contextSpaceRef?: unknown }).contextSpaceRef;
  if (!orgDomain || typeof space !== "string" || !space) {
    return undefined;
  }
  return `urn:ngsi-ld:Endpoint:${orgDomain}:${space}:${endpoint.metadata.name}`;
}

/**
 * The manifest a form produces (PL-04, PL-31, PL-33, PL-39). `base` is the manifest being
 * edited: what the form does not show (`enabled`, set by the pause button) survives an edit.
 */
export function toEnvelope(project: string, form: PipelineForm, base?: Manifest) {
  const { name, title, source, compute, allowFeedback, secretRefs, quotas, ...rest } = form;
  const enabled = base?.spec.enabled;
  const spec = prune({
    class: rest.class,
    schedule: rest.schedule,
    period: rest.period,
    source: source
      ? {
          ...source,
          dataSourceRef: typedRef("DataSource", source.dataSourceRef),
          endpointRef: typedRef("Endpoint", source.endpointRef),
        }
      : undefined,
    compute: compute
      ? { ...compute, mappingRef: typedRef("Mapping", compute.mappingRef) }
      : undefined,
    targetEndpoint: rest.targetEndpoint,
    output: rest.output,
    allowFeedback: allowFeedback ? true : undefined,
    secretRefs,
    quotas,
    ...(typeof enabled === "boolean" ? { enabled } : {}),
  });
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: {
      name: name ?? "",
      namespace: project,
      ...(title && Object.keys(title).length > 0 ? { title } : {}),
    },
    spec,
  };
}

/** The form one manifest fills, so editing starts from what is in Git rather than from blank. */
export function toForm(pipeline: Manifest): PipelineForm {
  const spec = pipeline.spec as Omit<PipelineForm, "name" | "title" | "source" | "compute"> & {
    source?: Record<string, unknown>;
    compute?: Record<string, unknown>;
    enabled?: boolean;
  };
  const { source, compute, enabled: _enabled, ...rest } = spec;
  void _enabled;
  return prune({
    name: pipeline.metadata.name,
    ...(pipeline.metadata.title ? { title: pipeline.metadata.title } : {}),
    ...rest,
    class: typeof rest.class === "string" ? rest.class : "auto",
    source: source
      ? {
          ...source,
          dataSourceRef: refName(source.dataSourceRef),
          endpointRef: refName(source.endpointRef),
        }
      : undefined,
    compute: compute ? { ...compute, mappingRef: refName(compute.mappingRef) } : undefined,
  }) as PipelineForm;
}

/** The manifest of a YAML document, read back into the form (the reverse of `toEnvelope`). */
export function fromManifest(document: unknown): PipelineForm {
  const manifest = document as Partial<Manifest>;
  if (!manifest.metadata || typeof manifest.metadata.name !== "string") {
    throw new Error("metadata.name is required");
  }
  return toForm({
    apiVersion: manifest.apiVersion ?? "joinedcontext.com/v1alpha1",
    kind: manifest.kind ?? "Pipeline",
    metadata: manifest.metadata,
    spec: (manifest.spec ?? {}) as Record<string, unknown>,
  });
}

/** The known values plus the one already chosen, so an edit never loses its own reference. */
function withCurrent(values: string[], current: string | undefined): string[] {
  return current && !values.includes(current) ? [...values, current] : values;
}

export interface PipelineEditorDialogProps {
  project: string;
  /** Called with `false` when the author closes the dialog; the page unmounts it then. */
  onOpenChange: (open: boolean) => void;
  /** The manifest being edited, or `null` for a new pipeline. */
  editing: Manifest | null;
  pending: boolean;
  error: string | null;
  onSubmit: (envelope: ReturnType<typeof toEnvelope>) => void;
}

/**
 * One dialog to create or edit a Pipeline: a form and the YAML it writes, both validated
 * against the same schema, proposed through the same change flow as every other write
 * (PL-04, UI-01, AP-13).
 *
 * Mounted only while it is open: the draft starts from the manifest being edited, or blank,
 * and a dialog closed and opened again starts over rather than showing the last draft.
 */
export function PipelineEditorDialog({
  project,
  onOpenChange,
  editing,
  pending,
  error,
  onSubmit,
}: PipelineEditorDialogProps): JSX.Element {
  const { t } = useTranslation();
  const { orgDomain } = useBranding();
  const [draft, setDraft] = useState<PipelineForm | undefined>(() =>
    editing ? toForm(editing) : undefined,
  );

  const dataSources = useQuery({
    queryKey: queryKeys.list(project, "datasources"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datasources" } },
        }),
      ),
  });
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const dataSourceNames = useMemo(
    () =>
      withCurrent(
        asManifests(dataSources.data?.items ?? []).map((source) => source.metadata.name),
        draft?.source?.dataSourceRef,
      ),
    [dataSources.data, draft?.source?.dataSourceRef],
  );
  const endpointOptions = useMemo<EndpointOption[]>(() => {
    const known = asManifests(endpoints.data?.items ?? []).map((endpoint) => ({
      name: endpoint.metadata.name,
      urn: endpointUrn(orgDomain, endpoint),
    }));
    const current = draft?.source?.endpointRef;
    const target = draft?.targetEndpoint;
    const names = known.map((endpoint) => endpoint.name);
    const urns = known.flatMap((endpoint) => (endpoint.urn ? [endpoint.urn] : []));
    return [
      ...known,
      ...(current && !names.includes(current) ? [{ name: current }] : []),
      ...(target && !urns.includes(target) ? [{ name: `urn:${target}`, urn: target }] : []),
    ];
  }, [endpoints.data, orgDomain, draft?.source?.endpointRef, draft?.targetEndpoint]);

  const schema = useMemo(
    () => pipelineSchema(t, dataSourceNames, endpointOptions),
    [t, dataSourceNames, endpointOptions],
  );
  const source = useMemo<ManifestSource<PipelineForm>>(
    () => ({
      toManifest: (form) => toEnvelope(project, form, editing ?? undefined),
      fromManifest,
    }),
    [project, editing],
  );

  // The inline Bloblang is not a manifest field: it is `bento.yaml` beside the manifest in
  // the forge (Architecture/08 §3), so the dialog says where to write it.
  const bentoUrl = editing?.status?.sourceUrl?.replace(/pipeline\.yaml$/, "bento.yaml");

  return (
    <ResourceFormDialog<PipelineForm>
      open
      onOpenChange={onOpenChange}
      size="lg"
      title={editing ? t("pipelines.dialog.edit") : t("pipelines.dialog.create")}
      description={t("pipelines.dialog.lead")}
      schema={schema}
      // A rename is a new manifest at a new path, so the name is fixed once it exists.
      uiSchema={editing ? { ...pipelineUiSchema, name: { "ui:readonly": true } } : pipelineUiSchema}
      formData={draft}
      submitLabel={t("pipelines.propose")}
      disabled={pending}
      error={error}
      source={source}
      onChange={setDraft}
      onSubmit={(form) => onSubmit(toEnvelope(project, form, editing ?? undefined))}
    >
      {draft?.compute?.kind === "bloblang" ? (
        <Alert
          tone="info"
          actions={
            bentoUrl ? (
              <a
                href={bentoUrl}
                target="_blank"
                rel="noreferrer"
                className={buttonClass("ghost", "sm", "text-primary")}
              >
                {t("pipelines.bloblangFile")}
                <Icon name="external" className="size-3.5" />
              </a>
            ) : undefined
          }
        >
          {t("pipelines.bloblangHint")}
        </Alert>
      ) : null}
    </ResourceFormDialog>
  );
}

export { PLURAL as PIPELINES_PLURAL };
