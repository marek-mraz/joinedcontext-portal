import { useCallback, useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parseGridConfig } from "@joinedcontext/sdk";
import type { ResolvedGridConfig, RichRow } from "@joinedcontext/sdk";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized, refName } from "../../api/manifest";
import { AccessPanel, deniedAttributes, useAccess } from "../../components/entities/AccessPanel";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { PortalEntityGrid } from "../../components/entities/PortalEntityGrid";
import { deleteEntity, fetchEntity, filterSlotsOf, useModelSource } from "../../components/entities/filters";
import type { EntityQuery } from "../../components/entities/filters";
import { Alert, Button, Dialog, Field, PageHeader, Select } from "../../components/ui";
import { writesOf } from "../access/EffectivePermissions";
import { entityTypesOf, pickReadEndpoint, spaceOf } from "../spaces/SpaceInside";

const PAGE_SIZE = 50;

function useProjectList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    // The key is shared with every other page that lists this kind, so the cache holds the
    // list as the API answers it, whichever page asked first; the manifests are read off it
    // here (T-0625).
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
    select: (list) => asManifests(list.items ?? []),
  });
}

/**
 * The data explorer (UI-33, UI-64): a space, one of its endpoints, filters generated from the
 * space's DataModel, and the entities the endpoint returns for the signed-in user, shown in the
 * one entity grid of the product. The page owns what is a page's business — the space and the
 * endpoint, the type and its filters, the export of what is on screen, and the removal of one
 * entity (UI-60) — and the grid owns the table: the columns, the metadata a person opens, the
 * paging, the per-column filter row and the correction of a value where the grant allows one.
 */
export function ExplorePage({
  project,
  initialSpace,
  initialEndpoint,
  initialEntityId,
}: {
  project: string;
  /** Chosen on arrival, the way a catalog card opens the page (UI-46). */
  initialSpace?: string;
  initialEndpoint?: string;
  /** One entity of the endpoint, opened on its detail as the page mounts (UI-46, T-1017). */
  initialEntityId?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const spaces = useProjectList(project, "spaces");
  const endpoints = useProjectList(project, "endpoints");
  const models = useProjectList(project, "datamodels");

  const [space, setSpace] = useState<string>(initialSpace ?? "");
  const [endpointChoice, setEndpointChoice] = useState<string>(initialEndpoint ?? "");
  const [query, setQuery] = useState<EntityQuery>({});
  const [selected, setSelected] = useState<string | null>(initialEntityId ?? null);
  const [removing, setRemoving] = useState(false);
  /** The page the grid holds right now, for the export: the rows on screen and where they start. */
  const [shown, setShown] = useState<{ rows: RichRow[]; offset: number }>({ rows: [], offset: 0 });
  /** Bumped when an entity is removed, so the grid reads the endpoint again. */
  const [generation, setGeneration] = useState(0);
  const queryClient = useQueryClient();

  const spaceEndpoints = (endpoints.data ?? []).filter((e) => spaceOf(e) === space);
  const endpoint =
    spaceEndpoints.find((e) => e.metadata.name === endpointChoice) ??
    pickReadEndpoint(spaceEndpoints);
  const slug = typeof endpoint?.spec.slug === "string" ? String(endpoint.spec.slug) : undefined;
  const spaceManifest = (spaces.data ?? []).find((s) => s.metadata.name === space);
  const model = (models.data ?? []).find(
    (m) => m.metadata.name === refName(spaceManifest?.spec.dataModelRef),
  );
  const types = model ? entityTypesOf(model) : [];
  // The slots and the denied list are memoized because the grid's config is built from them: a new
  // array on every render would make the grid rebuild its source and read the endpoint again, on
  // and on. It also stops the LinkML being parsed once per render.
  const modelSource = useModelSource(project, model);
  const slots = useMemo(() => filterSlotsOf(modelSource, query.type), [modelSource, query.type]);
  const access = useAccess(slug);
  const denied = useMemo(
    () => deniedAttributes(access.data, query.type, slots, t),
    [access.data, query.type, slots, t],
  );
  // UI-44, EP-55: a control the grant denies stays visible and says why, rather than working
  // until the gateway refuses it. The grant is the endpoint's own `/access` document, already
  // read above for the attributes it hides (T-1021).
  const mayDelete =
    access.data === undefined ||
    (access.data.permissions ?? []).some(
      (entry) =>
        (entry.resource?.type === undefined || entry.resource.type === query.type) &&
        (entry.actions ?? []).some((action) => action === "deleteEntity" || action === "deleteBatch"),
    );
  /**
   * The attributes this person may correct in place: the model's own slots, minus what the grant
   * hides, and only where the grant names a write on this type. Unlike the delete above, an edit
   * cell is offered only when the grant says yes — a cell that takes a value the endpoint will
   * refuse loses the person's typing, while a disabled button loses nothing (UI-67).
   */
  const editableAttrs = useMemo(() => {
    const grants = (access.data?.permissions ?? []).filter(
      (entry) =>
        (entry.resource?.type === undefined || entry.resource.type === query.type) &&
        writesOf(entry.actions ?? []).length > 0,
    );
    if (grants.length === 0) {
      return [];
    }
    const named = new Set(grants.flatMap((entry) => (Array.isArray(entry.attributes) ? entry.attributes : [])));
    const unlimited = grants.some((entry) => entry.attributes === "*" || entry.attributes === undefined);
    return slots
      .map((slot) => slot.name)
      .filter((attr) => !denied[attr] && (unlimited || named.has(attr)));
  }, [access.data, query.type, slots, denied]);

  /**
   * What the grid is told to show. The endpoint is a slug and never a URL (EP-55), and the filters
   * the page composed from the DataModel arrive as the grid's preset, so the grid's own filter row
   * narrows what the page already asked for rather than replacing it.
   */
  const config: ResolvedGridConfig | null = useMemo(() => {
    if (!slug || !query.type) {
      return null;
    }
    const parsed = parseGridConfig({
      source: { kind: "endpoint", slug },
      type: query.type,
      columns: (query.attrs ?? []).map((attr) => ({ attr })),
      filters: { preset: { q: query.q, attrs: query.attrs, scopeQ: query.scopeQ } },
      pageSize: PAGE_SIZE,
      history: { enabled: true },
      ...(editableAttrs.length > 0 ? { mode: "edit" as const, editableAttrs } : {}),
    });
    return parsed.config ?? null;
  }, [slug, query.type, query.q, query.attrs, query.scopeQ, editableAttrs]);

  // A stable callback and the same object back when nothing moved: the grid hands its page over
  // from an effect, so a new function or a new object on every render would read and re-render
  // without end.
  const onRows = useCallback((rows: RichRow[], offset: number) => {
    setShown((previous) =>
      previous.rows === rows && previous.offset === offset ? previous : { rows, offset },
    );
  }, []);
  const renderers = useMemo(
    () => ({
      // The identifier opens the entity as the endpoint holds it: the whole JSON-LD, and the one
      // place the removal of UI-60 is offered from.
      id: (_cell: unknown, row: RichRow) => (
        <button
          type="button"
          className="focus-ring font-mono text-primary underline-offset-2 hover:underline"
          onClick={() => setSelected(row.id)}
        >
          {row.id}
        </button>
      ),
    }),
    [],
  );

  const detail = useQuery({
    queryKey: ["explore-entity", slug, selected],
    queryFn: () => fetchEntity(slug!, selected!),
    enabled: Boolean(slug && selected),
  });

  // UI-60: the entity is removed through the same endpoint that shows it, with this person's
  // session. A refusal is the gateway's own sentence, shown as it came.
  const remove = useMutation({
    mutationFn: () => deleteEntity(slug!, selected!),
    onSuccess: () => {
      setRemoving(false);
      setSelected(null);
      void queryClient.removeQueries({ queryKey: ["explore-entity", slug] });
      // The grid holds a page that still lists the entity; a new key makes it read the endpoint
      // again rather than show a row that is gone.
      setGeneration((each) => each + 1);
    },
  });
  const removeFailed =
    remove.error instanceof ApiError ? remove.error.message : remove.error ? t("app.error.generic") : null;

  // UI-33: the page the person is looking at, as the file they can keep. It is written from
  // the rows already in hand rather than fetched again: a second read through the endpoint
  // would be a second answer, and a person exporting "this page" means this one.
  const download = () => {
    const entities = shown.rows.map((row) => row.raw);
    const file = new Blob([JSON.stringify(entities, null, 2)], { type: "application/json" });
    const href = URL.createObjectURL(file);
    const link = document.createElement("a");
    link.href = href;
    link.download = `${query.type}-${shown.offset + 1}-${shown.offset + entities.length}.json`;
    link.click();
    URL.revokeObjectURL(href);
  };

  function changeQuery(next: EntityQuery) {
    setQuery(next);
    setSelected(null);
  }

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title={t("explore.title")} description={t("explore.description")} />
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="explore-space" label={t("explore.space")}>
          <Select
            id="explore-space"
            value={space}
            onChange={(event) => {
              setSpace(event.target.value);
              setEndpointChoice("");
              changeQuery({});
            }}
          >
            <option value="">—</option>
            {(spaces.data ?? []).map((s) => (
              <option key={s.metadata.name} value={s.metadata.name}>
                {localized(s.metadata.title, locale, s.metadata.name)}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          id="explore-endpoint"
          label={t("explore.endpoint")}
          description={space && spaceEndpoints.length === 0 ? t("explore.noEndpoint") : undefined}
        >
          <Select
            id="explore-endpoint"
            value={endpoint?.metadata.name ?? ""}
            disabled={!space}
            onChange={(event) => {
              setEndpointChoice(event.target.value);
              setSelected(null);
            }}
          >
            <option value="">—</option>
            {spaceEndpoints.map((e) => (
              <option key={e.metadata.name} value={e.metadata.name}>
                {localized(e.metadata.title, locale, e.metadata.name)}
              </option>
            ))}
          </Select>
        </Field>
      </div>

      {space ? (
        <EntityFilters
          id="explore"
          types={types}
          slots={slots}
          value={query}
          onChange={changeQuery}
          denied={denied}
        />
      ) : null}
      {space ? <AccessPanel slug={slug} type={query.type} access={access} /> : null}

      {config ? (
        <PortalEntityGrid
          key={`${slug}-${query.type}-${generation}`}
          project={project}
          config={config}
          onRows={onRows}
          renderers={renderers}
          toolbar={
            <Button size="sm" disabled={shown.rows.length === 0} onClick={download}>
              {t("explore.export")}
            </Button>
          }
        />
      ) : space ? (
        <p className="text-caption text-fg-muted">{t("explore.noType")}</p>
      ) : null}

      {selected ? (
        <section
          aria-labelledby="explore-detail"
          className="rounded-md border border-border bg-surface p-3"
        >
          <div className="flex items-center justify-between gap-2">
            <h2 id="explore-detail" className="text-body font-semibold text-fg">
              {t("explore.detail")} <span className="font-mono font-normal">{selected}</span>
            </h2>
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant="danger"
                data-testid="explore-delete"
                disabled={!mayDelete}
                title={
                  mayDelete
                    ? undefined
                    : t("explore.deleteDenied", { type: query.type ?? "" })
                }
                onClick={() => {
                  remove.reset();
                  setRemoving(true);
                }}
              >
                {t("explore.delete")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setSelected(null)}>
                {t("explore.close")}
              </Button>
            </div>
          </div>
          {detail.error ? (
            <Alert role="alert" tone="danger">
              {detail.error instanceof ApiError ? detail.error.message : t("explore.loadFailed")}
            </Alert>
          ) : (
            <pre className="mt-2 max-h-96 overflow-auto text-caption" data-testid="explore-entity">
              {detail.data ? JSON.stringify(detail.data, null, 2) : t("app.loading")}
            </pre>
          )}
        </section>
      ) : null}

      <Dialog
        open={removing && selected !== null}
        onOpenChange={(next) => {
          setRemoving(next);
          if (!next) {
            remove.reset();
          }
        }}
        size="sm"
        title={t("explore.removeTitle")}
        description={t("explore.removeLead", { id: selected ?? "" })}
        closeLabel={t("explore.close")}
        footer={
          <>
            <Button variant="secondary" onClick={() => setRemoving(false)}>
              {t("form.cancel")}
            </Button>
            <Button
              variant="danger"
              disabled={remove.isPending}
              onClick={() => remove.mutate()}
              data-testid="explore-delete-confirm"
            >
              {t("explore.removeConfirm")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3">
          <p className="text-caption text-fg-muted">{t("explore.removeNote")}</p>
          {removeFailed ? (
            <Alert role="alert" tone="danger">
              {removeFailed}
            </Alert>
          ) : null}
        </div>
      </Dialog>
    </div>
  );
}
