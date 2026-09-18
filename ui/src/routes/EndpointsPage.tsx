import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, readCsrfToken, unwrap, whilePending } from "../api/client";
import { proposeChecked } from "../api/proposal";
import { asManifests, isChange, localized, overlay, plainTitle } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import type { Verdict } from "../api/drafts";
import { useOrgDomain, useProjects } from "../api/projects";
import { takePrefill } from "../assistant/state";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ChangeNotice } from "../components/ChangeNotice";
import { DeleteResourceAction } from "../components/DeleteResourceDialog";
import { ExportButton } from "../components/export/ExportButton";
import { SchemaProjectionPanel } from "../pages/endpoints/SchemaProjectionPanel";
import {
  ENDPOINT_LINKS,
  EndpointLink,
  endpointUrl,
  REPRESENTATION_PATHS,
} from "../components/endpoints/links";
import {
  admits,
  referenceManifest,
  referenceTo,
  SharedWithBadge,
  SPACE_LABEL,
  spaceOf,
} from "../components/endpoints/sharing";
import { endpointSchema, endpointUiSchema, generateSlug } from "../schemas/kinds";
import type { JsonSchema } from "../components/forms/types";
import { ModelPicker } from "../pages/endpoints/ModelPicker";
import type { ModelPickerState } from "../pages/endpoints/ModelPicker";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
  SourceLink,
} from "../components/ui";

interface EndpointForm {
  name: string;
  title?: string;
  contextSpaceRef: string;
  slug?: string;
  audience: string;
  enabledRepresentations: string[];
  allowedProjects?: string[];
  rateLimits?: { requestsPerMinute?: number; burst?: number };
  caching?: { maxAgeSeconds?: number };
}

function toSpec(
  form: EndpointForm,
  slug: string,
  hiddenAttributes: string[],
  projectionRefName?: string,
) {
  const { allowedProjects, rateLimits, caching, ...rest } = form;
  delete (rest as Partial<EndpointForm>).name;
  delete (rest as Partial<EndpointForm>).title;
  return {
    ...rest,
    slug,
    ...(form.audience === "project-list" && allowedProjects && allowedProjects.length > 0
      ? { allowedProjects }
      : {}),
    ...(typeof rateLimits?.requestsPerMinute === "number"
      ? {
          rateLimits: {
            requestsPerMinute: rateLimits.requestsPerMinute,
            ...(typeof rateLimits.burst === "number" ? { burst: rateLimits.burst } : {}),
          },
        }
      : {}),
    ...(typeof caching?.maxAgeSeconds === "number"
      ? { caching: { maxAgeSeconds: caching.maxAgeSeconds } }
      : {}),
    ...(hiddenAttributes.length > 0 ? { projection: { hiddenAttributes } } : {}),
    ...(projectionRefName
      ? { projectionRef: { kind: "ModelProjection", name: projectionRefName } }
      : {}),
  };
}

/** The spec keys the form writes; the rest of an edited manifest travels as it is (T-0885). */
const OWNED_SPEC = [
  "contextSpaceRef",
  "slug",
  "audience",
  "enabledRepresentations",
  "allowedProjects",
  "rateLimits",
  "caching",
  "projection",
  "projectionRef",
];

export function toEnvelope(
  project: string,
  form: EndpointForm,
  slug: string,
  hiddenAttributes: string[],
  projectionRefName?: string,
  base?: Manifest,
): Manifest {
  const { name, title } = form;
  const spec = toSpec(form, slug, hiddenAttributes, projectionRefName);
  const next: Manifest = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name,
      namespace: project,
      labels: { [SPACE_LABEL]: form.contextSpaceRef },
      ...(title?.trim() ? { title } : {}),
    },
    spec,
  };
  return overlay(base, next, OWNED_SPEC);
}

export function toForm(endpoint: Manifest): EndpointForm {
  const spec = endpoint.spec as {
    contextSpaceRef?: string;
    slug?: string;
    audience?: string;
    enabledRepresentations?: string[];
    allowedProjects?: string[];
    rateLimits?: { requestsPerMinute?: number; burst?: number };
    caching?: { maxAgeSeconds?: number };
  };
  return {
    name: endpoint.metadata.name,
    title: plainTitle(endpoint.metadata.title),
    contextSpaceRef: spec.contextSpaceRef ?? "",
    slug: spec.slug ?? "",
    audience: spec.audience ?? "project-list",
    enabledRepresentations: spec.enabledRepresentations ?? [],
    allowedProjects: spec.allowedProjects ?? [],
    rateLimits: spec.rateLimits,
    caching: spec.caching,
  };
}

function hiddenOf(endpoint: Manifest): string[] {
  const projection = (endpoint.spec as { projection?: { hiddenAttributes?: string[] } })
    .projection;
  return projection?.hiddenAttributes ?? [];
}

function CopyUrlButton({ slug }: { slug: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const url = endpointUrl(slug, "");

  return (
    <Button
      size="sm"
      title={url}
      icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
      onClick={() => {
        void navigator.clipboard
          ?.writeText(url)
          .then(() => setCopied(true))
          .catch(() => setCopied(false));
      }}
    >
      {copied ? t("endpoints.copied") : t("endpoints.copyUrl")}
    </Button>
  );
}

const COLUMNS = 5;
const SHARED_COLUMNS = 4;

/** `schema` without `allowedProjects`: the manifest refuses the list outside `project-list`. */
function withoutAllowedProjects(schema: JsonSchema): JsonSchema {
  const properties = Object.fromEntries(
    Object.entries(schema.properties ?? {}).filter(([key]) => key !== "allowedProjects"),
  );
  return { ...schema, properties };
}

/** Endpoints of one project: who may call them, in which representations, and their public URL. */
export function EndpointsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const [urlDraftName, setUrlDraftName] = useState(() => {
    if (typeof window === "undefined") return undefined;
    return new URLSearchParams(window.location.search).get("draft") ?? undefined;
  });

  const [activeSlug, setActiveSlug] = useState<string>(() => generateSlug());

  const [pickerState, setPickerState] = useState<ModelPickerState>({
    projectionName: "",
    classes: {},
  });

  // What the assistant hands the page: the form values, the attributes to hide, and `existing`
  // when it opened an endpoint that is already there rather than drafting a new one.
  const [prefill] = useState(
    () =>
      takePrefill(window.location.pathname) as
        | (Partial<EndpointForm> & { hiddenAttributes?: string[]; entityTypes?: string[]; existing?: boolean })
        | null,
  );
  const [editing, setEditing] = useState<EndpointForm | null>(() => {
    if (!urlDraftName && !prefill) {
      return null;
    }
    const form: Partial<EndpointForm> & { hiddenAttributes?: string[]; entityTypes?: string[]; existing?: boolean } = {
      ...prefill,
    };
    delete form.hiddenAttributes;
    delete form.entityTypes;
    delete form.existing;
    return {
      name: "",
      contextSpaceRef: "",
      audience: "project-list",
      enabledRepresentations: ["ngsi-ld"],
      allowedProjects: [],
      ...form,
      ...(urlDraftName ? { name: urlDraftName } : {}),
    };
  });
  const [isNew, setIsNew] = useState(
    prefill?.existing !== true && (prefill !== null || urlDraftName !== undefined),
  );
  // The classes the assistant's proposal exposes; none named means every class of the model.
  const [handedClasses] = useState(() =>
    prefill && prefill.existing !== true
      ? (prefill.entityTypes ?? []).filter((c): c is string => typeof c === "string")
      : undefined,
  );
  // The manifest the form edits, from the list or from the YAML view: what it does not show
  // travels with the proposal unchanged (T-0885).
  const [base, setBase] = useState<Manifest | null>(null);
  // The endpoint whose slug, hidden attributes and projection the page took over, once.
  const [adopted, setAdopted] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [hidden, setHidden] = useState<string[]>(() =>
    Array.isArray(prefill?.hiddenAttributes)
      ? prefill.hiddenAttributes.filter((a): a is string => typeof a === "string")
      : [],
  );

  const list = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  // An endpoint the assistant opened by name edits that endpoint: a PUT to its name with its own
  // slug, never a new endpoint with a fresh one. Only a draft or a prefill that says so is taken
  // over; a name typed into a new endpoint's form is left for the API to refuse.
  const openedName =
    urlDraftName !== undefined && (prefill === null || prefill.existing === true)
      ? urlDraftName
      : prefill?.existing === true && typeof prefill.name === "string"
        ? prefill.name
        : undefined;
  const opened = openedName
    ? asManifests(list.data?.items ?? []).find((endpoint) => endpoint.metadata.name === openedName)
    : undefined;
  if (opened && adopted !== opened.metadata.name) {
    setAdopted(opened.metadata.name);
    setIsNew(false);
    setBase(opened);
    const own = toForm(opened);
    const ownSlug = own.slug || (typeof prefill?.slug === "string" ? prefill.slug : "");
    if (ownSlug) {
      setActiveSlug(ownSlug);
    }
    if (!Array.isArray(prefill?.hiddenAttributes)) {
      setHidden(hiddenOf(opened));
    }
    const projectionRef = (opened.spec as { projectionRef?: { name?: string } })?.projectionRef?.name;
    setPickerState({
      projectionName: projectionRef || opened.metadata.name,
      selectedProjectionRef: projectionRef,
      classes: {},
    });
    // A draft the dialog loads replaces these; without one the form starts from the endpoint
    // as it is, with whatever the assistant changed on top.
    if (editing && !editing.contextSpaceRef) {
      const changed: Partial<EndpointForm> & { hiddenAttributes?: string[]; entityTypes?: string[]; existing?: boolean } = {
        ...(prefill?.existing === true ? prefill : {}),
      };
      delete changed.hiddenAttributes;
      delete changed.entityTypes;
      delete changed.existing;
      setEditing({ ...own, ...changed, name: own.name, slug: own.slug });
    }
  }

  const spacesQuery = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });

  const modelsQuery = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  // The other projects of the repository: what the allowed-project picker offers, and whose
  // endpoints the "shared with this project" section reads (PF-05, EP-14).
  const projects = useProjects();
  const others = (projects.data ?? []).filter((name) => name !== project);
  const otherLists = useQueries({
    queries: others.map((other) => ({
      queryKey: queryKeys.list(other, "endpoints"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project: other, plural: "endpoints" } },
          }),
        ),
    })),
  });
  const referencesQuery = useQuery({
    queryKey: queryKeys.list(project, "shared"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "shared" } },
        }),
      ),
  });

  const [verdict, setVerdict] = useState<Verdict | null>(null);

  // The policy's assigner is the organization, so the same domain the model editor mints under.
  const orgDomain = useOrgDomain(project);

  const editedSpace = editing?.contextSpaceRef;
  const spaceHasModel =
    editedSpace !== undefined &&
    asManifests(modelsQuery.data?.items ?? []).some(
      (m) => (m.spec as { contextSpaceRef?: string })?.contextSpaceRef === editedSpace,
    );

  const tickedClassNames = Object.keys(pickerState.classes).filter(
    (c) => pickerState.classes[c]?.ticked,
  );
  // In a space with a model a new endpoint publishes the ticked classes as its own projection
  // and policies, and so does an existing one whose classes are ticked; otherwise the endpoint is
  // proposed alone, with the projection it already names.
  const projecting =
    spaceHasModel && !pickerState.selectedProjectionRef && (isNew || tickedClassNames.length > 0);
  const projectionNameOf = (form: EndpointForm) =>
    pickerState.projectionName || form.name || "projection";

  /** The Endpoint the form stands for: what its draft holds, what is checked, what is proposed. */
  const endpointOf = (form: EndpointForm) =>
    toEnvelope(
      project,
      form,
      activeSlug,
      hidden,
      projecting ? projectionNameOf(form) : pickerState.selectedProjectionRef,
      base ?? undefined,
    );

  const buildManifests = (form: EndpointForm) => {
    const endpointEnvelope = endpointOf(form);
    if (!projecting) {
      return [endpointEnvelope];
    }
    const projectionName = projectionNameOf(form);

    const modelProjection = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ModelProjection",
      metadata: {
        name: projectionName,
        namespace: project,
        labels: { [SPACE_LABEL]: form.contextSpaceRef },
      },
      spec: {
        contextSpaceRef: form.contextSpaceRef,
        dataModelRef: {
          kind: "DataModel",
          name: pickerState.dataModelName ?? "",
          version: pickerState.dataModelVersion ?? "1",
        },
        classes: tickedClassNames.map((cName) => ({
          name: cName,
          slots: pickerState.classes[cName].slots,
        })),
      },
    };

    const readInfo = tickedClassNames.map((cName) => {
      const slots = pickerState.classes[cName].slots;
      return {
        entities: [{ type: cName }],
        ...(slots.length > 0 ? { propertyNames: slots } : {}),
      };
    });

    const readPolicy = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: {
        name: `${form.name}-read`,
        namespace: project,
        labels: { [SPACE_LABEL]: form.contextSpaceRef },
      },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: form.contextSpaceRef },
        assigner: `did:web:${orgDomain}`,
        assignee:
          form.audience === "public"
            ? { kind: "role", id: "public" }
            : { kind: "role", id: `${project}-readers` },
        operations: ["retrieveOps"],
        information: readInfo,
      },
    };

    const writableClassNames = tickedClassNames.filter((c) => pickerState.classes[c]?.writable);
    const manifests = [modelProjection, endpointEnvelope, readPolicy];

    if (writableClassNames.length > 0) {
      const writeInfo = writableClassNames.map((cName) => {
        const cfg = pickerState.classes[cName];
        return {
          entities: [{ type: cName, ...(cfg.idPattern ? { idPattern: cfg.idPattern } : {}) }],
        };
      });
      const firstCfg = pickerState.classes[writableClassNames[0]];
      const writePolicy = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Policy",
        metadata: {
          name: `${form.name}-write`,
          namespace: project,
          labels: { [SPACE_LABEL]: form.contextSpaceRef },
        },
        spec: {
          contextSpaceRef: { kind: "ContextSpace", name: form.contextSpaceRef },
          assigner: `did:web:${orgDomain}`,
          assignee: { kind: "role", id: `${project}-writers` },
          operations: ["updateOps"],
          information: writeInfo,
          ...(firstCfg?.scope ? { scopeQ: firstCfg.scope } : {}),
          ...(firstCfg?.writeQ ? { q: firstCfg.writeQ } : {}),
        },
      };
      manifests.push(writePolicy);
    }

    return manifests;
  };

  /** The projection, its policies and the endpoint as one import; `dryRun` checks without proposing. */
  const importBundle = async (form: EndpointForm, dryRun: boolean): Promise<unknown> => {
    if (tickedClassNames.length === 0) {
      throw new Error(t("endpoints.picker.nothingTicked"));
    }
    const res = await globalThis.fetch(
      new Request(
        `${window.location.origin}/api/v1/projects/${encodeURIComponent(project)}/import${dryRun ? "?dryRun=All" : ""}`,
        {
          method: "POST",
          credentials: "same-origin",
          // The import door mutates, so the session's double-submit token travels with it;
          // without the header every Check and Propose of a projected endpoint is a bare 403
          // and the dialog only says "not checked yet" (T-0898).
          headers: {
            "Content-Type": "application/json",
            "x-csrf-token": readCsrfToken() ?? "",
          },
          // The bundle is the form's own endpoint, its projection and its policies, not a
          // foreign import, so a projection an earlier proposal already drew is updated by this
          // one instead of refusing the whole check (MF-23, T-1227). Every manifest still lands
          // as a Change an approver reads, and "Show the manifests this proposes" says what.
          body: JSON.stringify({ manifests: buildManifests(form), conflictPolicy: "replace" }),
        },
      ),
    );
    if (!res.ok) {
      const problem = (await res.json().catch(() => null)) as { detail?: string } | null;
      throw new ApiError(res.status, problem?.detail || `HTTP ${res.status}`);
    }
    return res.json();
  };

  // A bundle is checked whole, and the endpoint is always checked with its draft too, so the
  // verdict the proposal waits for is recorded for exactly the manifest the form shows (AG-62).
  const check = useMutation({
    mutationFn: async (form: EndpointForm) => {
      setFormError(null);
      if (projecting) {
        await importBundle(form, true);
      }
      const envelope = endpointOf(form);
      const draftRef = form.name ? { kind: "Endpoint", name: form.name } : undefined;
      const body = (draftRef ? { ...envelope, draft: draftRef } : envelope) as never;
      return unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" }, query: { dryRun: "All" } },
          body,
        }),
      );
    },
    onSuccess: (result) => {
      const answer = result as { verdict?: Verdict };
      if (answer?.verdict) {
        setVerdict(answer.verdict);
      }
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  // The assistant's change opens checked: the form rebuilds the manifest from its fields, which
  // leaves the verdict the assistant recorded behind, so the page checks what it shows once.
  const checkedOnOpen = useRef(false);
  const openedOnChange = prefill?.existing === true && adopted !== null && modelsQuery.isSuccess;
  useEffect(() => {
    if (checkedOnOpen.current || !openedOnChange || !editing?.contextSpaceRef) {
      return;
    }
    checkedOnOpen.current = true;
    check.mutate(editing);
  }, [openedOnChange, editing, check]);

  const propose = useMutation({
    mutationFn: async ({
      form,
      create,
      draft: draftRef,
    }: {
      form: EndpointForm;
      create: boolean;
      draft?: { kind: string; name: string };
    }) => {
      setFormError(null);
      if (projecting) {
        return importBundle(form, false);
      }
      const envelope = endpointOf(form);
      const body = (draftRef ? { ...envelope, draft: draftRef } : envelope) as never;
      const result = create
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "endpoints" } },
            body,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural: "endpoints", name: form.name } },
            body,
          });
      return unwrap(result);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setEditing(null);
      setUrlDraftName(undefined);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "endpoints") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "projections") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  // One click declares the SharedSpaceReference; it lands as a Change like every write (EP-15).
  const reference = useMutation({
    mutationFn: async ({ source, endpoint }: { source: string; endpoint: Manifest }) =>
      proposeChecked(
        project,
        "shared",
        referenceManifest(project, source, endpoint) as { metadata: { name: string } },
        true,
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "shared") });
    },
  });

  const head = (
    <TableHead>
      <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("endpoints.field.audience")}</TableHeaderCell>
      <TableHeaderCell secondary>{t("endpoints.field.representations")}</TableHeaderCell>
      <TableHeaderCell>{t("endpoints.field.phase")}</TableHeaderCell>
      <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
    </TableHead>
  );

  if (list.isPending) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("endpoints.title")} description={t("endpoints.lead")} />
        <Table caption={t("endpoints.title")} status={t("app.loading")}>
          {head}
          <TableSkeleton columns={COLUMNS} />
        </Table>
      </div>
    );
  }

  if (list.isError) {
    const message =
      list.error instanceof ApiError
        ? (list.error.problem?.detail ?? list.error.message)
        : t("app.error.generic");
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("endpoints.title")} description={t("endpoints.lead")} />
        <Alert
          role="alert"
          tone="danger"
          actions={
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() => {
                void list.refetch();
              }}
            >
              {t("app.error.retry")}
            </Button>
          }
        >
          {message}
        </Alert>
      </div>
    );
  }

  const endpoints = asManifests(list.data.items ?? []);
  const spaceNames = asManifests(spacesQuery.data?.items ?? []).map((s) => s.metadata.name);
  const references = asManifests(referencesQuery.data?.items ?? []);
  const shared = others.flatMap((source, index) =>
    asManifests(otherLists[index]?.data?.items ?? [])
      .filter((endpoint) => admits(endpoint, source, project))
      .map((endpoint) => ({ source, endpoint })),
  );
  // A reference whose endpoint no longer shows above: unshared, deleted, or in a project this
  // person may not read. It stays visible here, because this is the only page that shows the
  // references now and a row nothing lists is a grant nobody can remove (T-0706, EP-15).
  const orphaned = references.filter((reference) => {
    const slug = (reference.spec as { endpointSlug?: string }).endpointSlug ?? "";
    return !shared.some(
      ({ endpoint }) => (endpoint.spec as { slug?: string }).slug === slug,
    );
  });
  const pickable =
    others.length > 0
      ? [
          ...others,
          ...(editing?.allowedProjects ?? []).filter(
            (name) => typeof name === "string" && name.length > 0 && !others.includes(name),
          ),
        ]
      : [];
  const baseSchema = endpointSchema(t, spaceNames, pickable, editing?.rateLimits?.requestsPerMinute);
  const schema =
    editing?.audience === "project-list" ? baseSchema : withoutAllowedProjects(baseSchema);
  const uiSchema = {
    ...endpointUiSchema,
    ...(pickable.length > 0 ? { allowedProjects: { "ui:widget": "checkboxes" } } : {}),
  };

  // Rebuilt on every render: a handful of small objects, and no hook after the early returns.
  const previewManifests = editing ? buildManifests(editing) : null;

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={t("endpoints.title")}
        description={t("endpoints.lead")}
        actions={
          <PermissionGuard project={project} kind="Endpoint" verb="propose">
            <Button
              variant="primary"
              icon={<Icon name="plus" className="size-4" />}
              onClick={() => {
                setFormError(null);
                setIsNew(true);
                setBase(null);
                setUrlDraftName(undefined);
                setHidden([]);
                const newSlug = generateSlug();
                setActiveSlug(newSlug);
                setPickerState({ projectionName: "", classes: {} });
                setEditing({
                  name: "",
                  contextSpaceRef: spaceNames[0] ?? "",
                  audience: "project-list",
                  enabledRepresentations: ["ngsi-ld"],
                  allowedProjects: [],
                });
              }}
            >
              {t("endpoints.add")}
            </Button>
          </PermissionGuard>
        }
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <Table caption={t("endpoints.title")}>
        {head}
        <TableBody>
          {endpoints.length === 0 ? (
            <TableEmpty columns={COLUMNS}>
              <EmptyState
                bare
                icon="endpoints"
                title={t("endpoints.empty")}
                description={t("endpoints.addHint")}
              />
            </TableEmpty>
          ) : (
            endpoints.map((endpoint) => {
              const spec = endpoint.spec as {
                slug?: string;
                enabledRepresentations?: string[];
              };
              const slug = spec.slug ?? "";
              return (
                <TableRow key={endpoint.metadata.name}>
                  <TableCell primary>
                    <div>{localized(endpoint.metadata.title, locale, endpoint.metadata.name)}</div>
                    {endpoint.metadata.title ? (
                      <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                        {endpoint.metadata.name}
                      </div>
                    ) : null}
                  </TableCell>
                  <TableCell secondary>
                    <SharedWithBadge endpoint={endpoint} />
                  </TableCell>
                  <TableCell secondary>
                    <ul className="flex flex-wrap gap-1">
                      {(spec.enabledRepresentations ?? []).map((rep) => (
                        <li key={rep}>
                          {slug && REPRESENTATION_PATHS[rep] ? (
                            <EndpointLink href={endpointUrl(slug, REPRESENTATION_PATHS[rep])}>
                              {rep}
                            </EndpointLink>
                          ) : (
                            <Badge mono>{rep}</Badge>
                          )}
                        </li>
                      ))}
                    </ul>
                    {slug ? (
                      <ul className="mt-1.5 flex flex-wrap gap-1">
                        {ENDPOINT_LINKS.map((link) => (
                          <li key={link.key}>
                            <EndpointLink muted href={endpointUrl(slug, link.path)}>
                              {t(`endpoints.link.${link.key}`)}
                            </EndpointLink>
                          </li>
                        ))}
                      </ul>
                    ) : null}
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge kind="phase" value={endpoint.status?.phase} />
                  </TableCell>
                  <TableCell align="right">
                    <div className="flex flex-wrap items-center justify-end gap-1.5">
                      {spec.slug ? <CopyUrlButton slug={spec.slug} /> : null}
                      <ExportButton
                        project={project}
                        target={{ plural: "endpoints", name: endpoint.metadata.name }}
                        label={t("export.action")}
                        size="sm"
                      />
                      <PermissionGuard project={project} kind="Endpoint" verb="propose">
                        <Button
                          size="sm"
                          onClick={() => {
                            setFormError(null);
                            setIsNew(false);
                            setBase(endpoint);
                            setUrlDraftName(endpoint.metadata.name);
                            setHidden(hiddenOf(endpoint));
                            const parsedForm = toForm(endpoint);
                            setActiveSlug(parsedForm.slug || generateSlug());
                            const pRefName = (endpoint.spec as { projectionRef?: { name?: string } })
                              ?.projectionRef?.name;
                            setPickerState({
                              projectionName: pRefName || endpoint.metadata.name,
                              selectedProjectionRef: pRefName,
                              classes: {},
                            });
                            setEditing(parsedForm);
                          }}
                        >
                          {t("endpoints.edit")}
                        </Button>
                      </PermissionGuard>
                      <DeleteResourceAction
                        target={{ project, kind: "Endpoint", plural: "endpoints", name: endpoint.metadata.name }}
                      />
                      {endpoint.status?.sourceUrl ? (
                        <SourceLink href={endpoint.status.sourceUrl} label={t("spaces.field.source")} />
                      ) : null}
                    </div>
                  </TableCell>
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>

      <section aria-labelledby="shared-with-project" className="flex flex-col gap-3">
        <div>
          <h2 id="shared-with-project" className="text-title font-semibold text-fg">
            {t("endpoints.shared.title")}
          </h2>
          <p className="mt-1 text-body text-fg-muted">{t("endpoints.shared.lead")}</p>
        </div>
        <Table
          caption={t("endpoints.shared.title")}
          status={
            projects.isPending || otherLists.some((other) => other.isPending)
              ? t("app.loading")
              : undefined
          }
        >
          <TableHead>
            <TableHeaderCell secondary>{t("endpoints.shared.source")}</TableHeaderCell>
            <TableHeaderCell>{t("endpoints.field.name")}</TableHeaderCell>
            <TableHeaderCell secondary>{t("endpoints.field.representations")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("endpoints.shared.reference")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {shared.length === 0 && orphaned.length === 0 ? (
              <TableEmpty columns={SHARED_COLUMNS}>
                <EmptyState bare icon="globe" title={t("endpoints.shared.empty")} />
              </TableEmpty>
            ) : (
              shared.map(({ source, endpoint }) => {
                const spec = endpoint.spec as { slug?: string; enabledRepresentations?: string[] };
                const slug = spec.slug ?? "";
                const space = spaceOf(endpoint);
                const declared = slug ? referenceTo(references, slug) : undefined;
                const label = localized(endpoint.metadata.title, locale, endpoint.metadata.name);
                return (
                  <TableRow key={`${source}/${endpoint.metadata.name}`}>
                    <TableCell secondary>
                      <div className="flex flex-col gap-0.5">
                        <Link
                          to="/projects/$project/$plural"
                          params={{ project: source, plural: "endpoints" }}
                          className="focus-ring rounded-sm font-medium text-fg hover:underline"
                        >
                          {source}
                        </Link>
                        {space ? (
                          <Link
                            to="/projects/$project/spaces/$name"
                            params={{ project: source, name: space }}
                            className="focus-ring inline-flex items-center gap-1 rounded-sm font-mono text-caption text-primary hover:underline"
                          >
                            {space}
                            <Icon name="chevronRight" className="size-3.5" />
                          </Link>
                        ) : null}
                      </div>
                    </TableCell>
                    <TableCell primary>
                      <div>{label}</div>
                      {endpoint.metadata.title ? (
                        <div className="mt-0.5 font-mono text-caption text-fg-subtle">
                          {endpoint.metadata.name}
                        </div>
                      ) : null}
                      <div className="mt-1">
                        <SharedWithBadge endpoint={endpoint} />
                      </div>
                    </TableCell>
                    <TableCell secondary>
                      <ul className="flex flex-wrap gap-1">
                        {(spec.enabledRepresentations ?? []).map((rep) => (
                          <li key={rep}>
                            {slug && REPRESENTATION_PATHS[rep] ? (
                              <EndpointLink href={endpointUrl(slug, REPRESENTATION_PATHS[rep])}>
                                {rep}
                              </EndpointLink>
                            ) : (
                              <Badge mono>{rep}</Badge>
                            )}
                          </li>
                        ))}
                      </ul>
                      {slug ? (
                        <ul className="mt-1.5 flex flex-wrap gap-1">
                          {ENDPOINT_LINKS.map((link) => (
                            <li key={link.key}>
                              <EndpointLink muted href={endpointUrl(slug, link.path)}>
                                {t(`endpoints.link.${link.key}`)}
                              </EndpointLink>
                            </li>
                          ))}
                        </ul>
                      ) : null}
                    </TableCell>
                    <TableCell align="right">
                      {declared ? (
                        <div className="flex flex-col items-end gap-1">
                          <Badge tone="success">{t("endpoints.shared.referenced")}</Badge>
                          <span className="font-mono text-caption text-fg-muted">
                            {t("endpoints.shared.alias")}:{" "}
                            {String((declared.spec as { alias?: string }).alias ?? "")}
                          </span>
                          <DeleteResourceAction
                            target={{
                              project,
                              kind: "SharedSpaceReference",
                              plural: "shared",
                              name: declared.metadata.name,
                              label: `${source}/${endpoint.metadata.name}`,
                            }}
                          />
                        </div>
                      ) : (
                        <Button
                          size="sm"
                          variant="primary"
                          aria-label={`${t("endpoints.shared.use")}: ${source}/${endpoint.metadata.name}`}
                          disabled={!slug || reference.isPending}
                          icon={<Icon name="plus" className="size-4" />}
                          onClick={() => reference.mutate({ source, endpoint })}
                        >
                          {t("endpoints.shared.use")}
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })
            )}
            {orphaned.map((reference) => {
              const alias = String((reference.spec as { alias?: string }).alias ?? "");
              return (
                <TableRow key={`orphan/${reference.metadata.name}`}>
                  <TableCell>
                    <span className="text-fg-muted">{t("endpoints.shared.goneSource")}</span>
                  </TableCell>
                  <TableCell primary>
                    <div>{reference.metadata.name}</div>
                    <div className="mt-1">
                      <Badge tone="warning">{t("endpoints.shared.gone")}</Badge>
                    </div>
                  </TableCell>
                  <TableCell>
                    <span className="font-mono text-caption text-fg-muted">
                      {t("endpoints.shared.alias")}: {alias}
                    </span>
                  </TableCell>
                  <TableCell align="right">
                    <DeleteResourceAction
                      target={{
                        project,
                        kind: "SharedSpaceReference",
                        plural: "shared",
                        name: reference.metadata.name,
                        label: alias || reference.metadata.name,
                      }}
                    />
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
        {reference.isError ? (
          <Alert role="alert" tone="danger">
            {reference.error instanceof ApiError
              ? (reference.error.problem?.detail ?? reference.error.message)
              : t("app.error.generic")}
          </Alert>
        ) : null}
      </section>

      <ResourceFormDialog<EndpointForm>
        kind="Endpoint"
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setEditing(null);
            setUrlDraftName(undefined);
          }
        }}
        project={project}
        draftKind="Endpoint"
        draftName={editing?.name || urlDraftName || undefined}
        verdict={verdict}
        onVerdictChange={setVerdict}
        onCheck={(form) => {
          if (form.name) {
            check.mutate(form);
          }
        }}
        source={{
          toManifest: endpointOf,
          fromManifest: (manifest) => {
            setBase(manifest as Manifest);
            return toForm(manifest as Manifest);
          },
        }}
        title={isNew ? t("endpoints.add") : t("endpoints.edit")}
        description={t("endpoints.addHint")}
        schema={schema}
        uiSchema={uiSchema}
        formData={editing ?? undefined}
        submitLabel={t("endpoints.propose")}
        submitting={propose.isPending}
        error={formError}
        onSubmit={(form, draftRef) => propose.mutate({ form, create: isNew, draft: draftRef })}
        onChange={(form) => {
          if (form) {
            setEditing(form);
          }
        }}
        afterFields={
          <>
            {/* UI-58, T-0958: a form the assistant filled says so, so the person reads the
                values as a proposal to check rather than as something they typed. */}
            {prefill ? (
              <Alert role="note" tone="info">
                {t("assistant.prefilledNotice")}
              </Alert>
            ) : null}

            {editing?.audience === "public" ? (
              <Alert role="note" tone="warning">
                {t("endpoints.publicNotice")}
              </Alert>
            ) : null}

            {/* What the endpoint publishes is its purpose, so the picker is open, never behind a
                click, with what is ticked said above it (T-1389). */}
            {editing?.contextSpaceRef ? (
              <section className="rounded border border-border p-3" aria-labelledby="endpoint-projection">
                <h3 id="endpoint-projection" className="text-body font-medium text-fg">
                  {t("endpoints.section.projection")}
                </h3>
                <p data-testid="projection-summary" className="text-caption text-fg-muted">
                  {t("endpoints.projection.summary", {
                    types: Object.values(pickerState.classes).filter((config) => config.ticked).length,
                    attributes: Object.values(pickerState.classes)
                      .filter((config) => config.ticked)
                      .reduce((sum, config) => sum + config.slots.length, 0),
                  })}
                </p>
                <div className="mt-3">
                  <ModelPicker
                    handed={handedClasses}
                    project={project}
                    spaceName={editing.contextSpaceRef}
                    endpointName={editing.name}
                    disabled={propose.isPending}
                    value={pickerState}
                    onChange={setPickerState}
                  />
                </div>
              </section>
            ) : null}

            {/* A new endpoint publishes nothing yet, so there is no schema to hide attributes of. */}
            {!isNew && activeSlug ? (
              <details className="rounded border border-border p-3">
                <summary className="cursor-pointer text-body font-medium text-fg">
                  {t("endpoints.section.hidden")}
                </summary>
                <div className="mt-3">
                  <SchemaProjectionPanel slug={activeSlug} hidden={hidden} onHiddenChange={setHidden} />
                </div>
              </details>
            ) : null}

            {previewManifests && editing?.name ? (
              <details className="rounded border border-border p-3">
                <summary className="cursor-pointer text-body font-medium text-fg">
                  {t("endpoints.section.manifests")}
                </summary>
                <pre
                  data-testid="endpoint-preview"
                  className="mt-3 max-h-72 overflow-auto rounded border border-border bg-surface p-2.5 font-mono text-xs text-fg-muted"
                >
                  {JSON.stringify(previewManifests, null, 2)}
                </pre>
              </details>
            ) : null}
          </>
        }
      >
        <div className="flex flex-col gap-1 rounded border border-border bg-surface-subtle px-3 py-2">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-caption font-medium text-fg">{t("endpoints.field.publicUrl")}</span>
            <code data-testid="endpoint-url" className="min-w-0 break-all font-mono text-caption text-fg-muted">
              {`${window.location.origin}/api/endpoint/`}
              <span data-testid="endpoint-slug" className="font-semibold text-fg">
                {activeSlug}
              </span>
            </code>
            <CopyUrlButton slug={activeSlug} />
          </div>
          <p className="text-xs text-fg-subtle">{t("endpoints.slugHint")}</p>
        </div>
      </ResourceFormDialog>
    </div>
  );
}
