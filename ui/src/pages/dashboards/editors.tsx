import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { isChange, prune } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { filterSlotsOf } from "../../components/entities/filters";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { dashboardSchema, dashboardUiSchema, layerSchema, layerUiSchema } from "../../schemas/kinds";
import { entityTypesOf, spaceOf } from "../spaces/SpaceInside";

const API_VERSION = "joinedcontext.com/v1alpha1";

export interface DashboardForm {
  name: string;
  title: Record<string, string>;
  visibility: string;
  pages: {
    title?: string;
    layout?: string;
    layers?: string[];
    widgets?: { widgetType: string; endpointRef?: string; entityId?: string; property?: string }[];
  }[];
}

export interface LayerForm {
  name: string;
  sourceEndpointRef: string;
  entityType: string;
  style: string;
  visible?: boolean;
  filter?: { q?: string; scopeQ?: string; geoQ?: string };
  colorBy?: { property?: string; palette?: string; domain?: number[] };
  sizeBy?: { property?: string; range?: number[] };
  popupProperties?: string[];
}

function envelope(project: string, kind: string, name: string, spec: unknown) {
  return { apiVersion: API_VERSION, kind, metadata: { name, namespace: project }, spec: prune(spec) };
}

export function dashboardToManifest(project: string, form: DashboardForm) {
  const { name, ...spec } = form;
  return envelope(project, "Dashboard", name, spec);
}

export function dashboardFromManifest(document: unknown): DashboardForm {
  const manifest = (document ?? {}) as Partial<Manifest>;
  const spec = (manifest.spec ?? {}) as Omit<DashboardForm, "name">;
  return {
    name: manifest.metadata?.name ?? "",
    title: spec.title ?? {},
    visibility: spec.visibility ?? "project",
    pages: spec.pages ?? [{ layout: "full-map", layers: [] }],
  };
}

export function layerToManifest(project: string, form: LayerForm) {
  const { name, visible, ...spec } = form;
  // `true` is the default and is not written; `false` is the whole point of the member.
  return envelope(project, "Layer", name, { ...spec, ...(visible === false ? { visible } : {}) });
}

export function layerFromManifest(document: unknown): LayerForm {
  const manifest = (document ?? {}) as Partial<Manifest>;
  const spec = (manifest.spec ?? {}) as Omit<LayerForm, "name">;
  return {
    name: manifest.metadata?.name ?? "",
    sourceEndpointRef: spec.sourceEndpointRef ?? "",
    entityType: spec.entityType ?? "",
    style: spec.style ?? "circle",
    visible: spec.visible ?? true,
    filter: spec.filter,
    colorBy: spec.colorBy,
    sizeBy: spec.sizeBy,
    popupProperties: spec.popupProperties,
  };
}

/** One PUT or POST of a manifest, as the endpoints page does it; the result is a Change. */
function useProposal(project: string, plural: string, onChange: (change: Change) => void) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: async ({ body, create }: { body: { metadata: { name: string } }; create: boolean }) => {
      setError(null);
      const result = create
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural } },
            body: body as never,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural, name: body.metadata.name } },
            body: body as never,
          });
      return unwrap(result);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        onChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, plural) });
    },
    onError: (err) => {
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });
  return { mutation, error, reset: () => setError(null) };
}

export interface EditorProps<T> {
  project: string;
  /** The form being edited; `null` keeps the dialog closed. */
  editing: T | null;
  isNew: boolean;
  onEditingChange: (form: T | null) => void;
  onChange: (change: Change) => void;
}

export function DashboardEditor({
  project,
  editing,
  isNew,
  onEditingChange,
  onChange,
  layers,
}: EditorProps<DashboardForm> & { layers: string[] }): JSX.Element {
  const { t } = useTranslation();
  const proposal = useProposal(project, "dashboards", (change) => {
    onChange(change);
    onEditingChange(null);
  });
  const schema = useMemo(() => dashboardSchema(t, layers), [t, layers]);
  return (
    <ResourceFormDialog<DashboardForm>
      open={editing !== null}
      onOpenChange={(open) => {
        if (!open) {
          onEditingChange(null);
          proposal.reset();
        }
      }}
      title={isNew ? t("dashboards.add") : t("dashboards.edit")}
      description={t("dashboards.addHint")}
      schema={schema}
      uiSchema={dashboardUiSchema}
      formData={editing ?? undefined}
      submitLabel={t("dashboards.propose")}
      disabled={proposal.mutation.isPending}
      error={proposal.error}
      size="lg"
      source={{
        toManifest: (form) => dashboardToManifest(project, form),
        fromManifest: dashboardFromManifest,
      }}
      onSubmit={(form) => proposal.mutation.mutate({ body: dashboardToManifest(project, form), create: isNew })}
      onChange={(form) => {
        if (form) {
          onEditingChange(form);
        }
      }}
    />
  );
}

export function LayerEditor({
  project,
  editing,
  isNew,
  onEditingChange,
  onChange,
  endpoints,
  spaces,
  models,
}: EditorProps<LayerForm> & { endpoints: Manifest[]; spaces: Manifest[]; models: Manifest[] }): JSX.Element {
  const { t } = useTranslation();
  const proposal = useProposal(project, "layers", (change) => {
    onChange(change);
    onEditingChange(null);
  });
  // The type and the filter come from the space the chosen endpoint publishes (UI-33): its
  // DataModel gives the types and the typed attributes the filter rows offer.
  const endpoint = endpoints.find((e) => e.metadata.name === editing?.sourceEndpointRef);
  const space = endpoint ? spaces.find((s) => s.metadata.name === spaceOf(endpoint)) : undefined;
  const model = models.find((m) => m.metadata.name === space?.spec.dataModelRef);
  const types = useMemo(() => (model ? entityTypesOf(model) : []), [model]);
  const slots = filterSlotsOf(model, editing?.entityType || undefined);
  const endpointNames = useMemo(() => endpoints.map((e) => e.metadata.name), [endpoints]);
  const schema = useMemo(() => layerSchema(t, endpointNames, types), [t, endpointNames, types]);

  return (
    <ResourceFormDialog<LayerForm>
      open={editing !== null}
      onOpenChange={(open) => {
        if (!open) {
          onEditingChange(null);
          proposal.reset();
        }
      }}
      title={isNew ? t("dashboards.addLayer") : t("dashboards.editLayer")}
      description={t("dashboards.layerHint")}
      schema={schema}
      uiSchema={layerUiSchema}
      formData={editing ?? undefined}
      submitLabel={t("dashboards.propose")}
      disabled={proposal.mutation.isPending}
      error={proposal.error}
      size="lg"
      source={{
        toManifest: (form) => layerToManifest(project, form),
        fromManifest: layerFromManifest,
      }}
      onSubmit={(form) => proposal.mutation.mutate({ body: layerToManifest(project, form), create: isNew })}
      onChange={(form) => {
        if (form) {
          onEditingChange(form);
        }
      }}
    >
      {editing && endpoint ? (
        <EntityFilters
          id="layer"
          types={types}
          slots={slots}
          value={{
            type: editing.entityType || undefined,
            q: editing.filter?.q,
            scopeQ: editing.filter?.scopeQ,
            attrs: editing.popupProperties,
          }}
          onChange={(query) =>
            onEditingChange({
              ...editing,
              entityType: query.type ?? "",
              filter: prune({ ...editing.filter, q: query.q, scopeQ: query.scopeQ }),
              popupProperties: query.attrs,
            })
          }
        />
      ) : null}
    </ResourceFormDialog>
  );
}
