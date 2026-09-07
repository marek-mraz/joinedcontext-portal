import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import { AccessPanel, deniedAttributes, useAccess } from "../../components/entities/AccessPanel";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { fetchEntities, fetchEntity, filterSlotsOf } from "../../components/entities/filters";
import type { EntityQuery } from "../../components/entities/filters";
import {
  Alert,
  Button,
  Field,
  PageHeader,
  Select,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { entityTypesOf, pickReadEndpoint, spaceOf } from "../spaces/SpaceInside";

const PAGE_SIZES = [20, 50, 100];
/** Columns beyond this hide behind the detail pane: a wide table reads worse than a JSON. */
const MAX_COLUMNS = 8;

function useProjectList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    queryFn: async () =>
      asManifests(
        (
          await unwrap(
            await api.GET("/api/v1/projects/{project}/{plural}", {
              params: { path: { project, plural } },
            }),
          )
        ).items ?? [],
      ),
  });
}

function cell(value: unknown): string {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > 40 ? `${text.slice(0, 39)}…` : text;
}

/**
 * The data explorer (UI-33): a space, one of its endpoints, filters generated from the space's
 * DataModel, and the page of entities the endpoint returns for the signed-in user. Reads only.
 */
export function ExplorePage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const spaces = useProjectList(project, "spaces");
  const endpoints = useProjectList(project, "endpoints");
  const models = useProjectList(project, "datamodels");

  const [space, setSpace] = useState<string>("");
  const [endpointChoice, setEndpointChoice] = useState<string>("");
  const [query, setQuery] = useState<EntityQuery>({});
  const [limit, setLimit] = useState(PAGE_SIZES[0]);
  const [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);

  const spaceEndpoints = (endpoints.data ?? []).filter((e) => spaceOf(e) === space);
  const endpoint =
    spaceEndpoints.find((e) => e.metadata.name === endpointChoice) ?? pickReadEndpoint(spaceEndpoints);
  const slug = typeof endpoint?.spec.slug === "string" ? (endpoint.spec.slug as string) : undefined;
  const spaceManifest = (spaces.data ?? []).find((s) => s.metadata.name === space);
  const model = (models.data ?? []).find((m) => m.metadata.name === spaceManifest?.spec.dataModelRef);
  const types = model ? entityTypesOf(model) : [];
  const slots = filterSlotsOf(model, query.type);
  const access = useAccess(slug);
  const denied = deniedAttributes(access.data, query.type, slots, t);

  const page = useQuery({
    queryKey: ["explore", slug, query, limit, offset],
    queryFn: () => fetchEntities(slug!, query, { limit, offset, count: true }),
    enabled: Boolean(slug && query.type),
    placeholderData: (previous) => previous,
  });
  const detail = useQuery({
    queryKey: ["explore-entity", slug, selected],
    queryFn: () => fetchEntity(slug!, selected!),
    enabled: Boolean(slug && selected),
  });

  const rows = page.data?.rows ?? [];
  const columns =
    query.attrs && query.attrs.length > 0
      ? query.attrs
      : [...new Set(rows.flatMap((row) => Object.keys(row)))]
          .filter((key) => !["id", "type", "@context"].includes(key))
          .slice(0, MAX_COLUMNS);
  const count = page.data?.count;
  const hasNext = count !== undefined ? offset + limit < count : rows.length === limit;

  function changeQuery(next: EntityQuery) {
    setQuery(next);
    setOffset(0);
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
              setOffset(0);
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
        <EntityFilters id="explore" types={types} slots={slots} value={query} onChange={changeQuery} denied={denied} />
      ) : null}
      {space ? <AccessPanel slug={slug} type={query.type} access={access} /> : null}

      {page.error ? (
        <Alert role="alert" tone="danger">
          {page.error instanceof ApiError ? page.error.message : t("explore.loadFailed")}
        </Alert>
      ) : null}

      {slug && query.type ? (
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-3 text-caption text-fg-muted">
            <span>{count !== undefined ? t("explore.count", { count }) : t("explore.countUnknown")}</span>
            <span>{t("explore.page", { from: offset + 1, to: offset + rows.length })}</span>
            <label className="inline-flex items-center gap-1">
              {t("explore.pageSize")}
              <Select
                value={String(limit)}
                className="w-20"
                onChange={(event) => {
                  setLimit(Number(event.target.value));
                  setOffset(0);
                }}
              >
                {PAGE_SIZES.map((size) => (
                  <option key={size} value={String(size)}>
                    {size}
                  </option>
                ))}
              </Select>
            </label>
            <Button size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - limit))}>
              {t("explore.prev")}
            </Button>
            <Button size="sm" disabled={!hasNext} onClick={() => setOffset(offset + limit)}>
              {t("explore.next")}
            </Button>
          </div>
          <div className="overflow-x-auto">
            <Table caption={t("explore.entities")}>
              <TableHead>
                <TableRow>
                  <TableHeaderCell>id</TableHeaderCell>
                  {columns.map((column) => (
                    <TableHeaderCell key={column}>
                      <span className="font-mono">{column}</span>
                    </TableHeaderCell>
                  ))}
                </TableRow>
              </TableHead>
              <TableBody>
                {rows.length === 0 && !page.isPending ? (
                  <TableEmpty columns={columns.length + 1}>{t("explore.noEntities")}</TableEmpty>
                ) : (
                  rows.map((row) => (
                    <TableRow key={row.id}>
                      <TableCell>
                        <button
                          type="button"
                          className="focus-ring font-mono text-primary underline-offset-2 hover:underline"
                          onClick={() => setSelected(row.id)}
                        >
                          {row.id}
                        </button>
                      </TableCell>
                      {columns.map((column) => (
                        <TableCell key={column}>
                          <span className="font-mono">{column in row ? cell(row[column]) : ""}</span>
                        </TableCell>
                      ))}
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        </div>
      ) : space ? (
        <p className="text-caption text-fg-muted">{t("explore.noType")}</p>
      ) : null}

      {selected ? (
        <section aria-labelledby="explore-detail" className="rounded-md border border-border bg-surface p-3">
          <div className="flex items-center justify-between gap-2">
            <h2 id="explore-detail" className="text-body font-semibold text-fg">
              {t("explore.detail")} <span className="font-mono font-normal">{selected}</span>
            </h2>
            <Button size="sm" variant="ghost" onClick={() => setSelected(null)}>
              {t("explore.close")}
            </Button>
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
    </div>
  );
}
