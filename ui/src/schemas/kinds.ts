import { parse as parseYaml } from "yaml";
import type { JsonSchema, UiSchema } from "../components/forms/types";

/**
 * Draft-07 schemas for the kinds the Portal writes, mirroring jc-core's `ContextSpaceSpec`
 * and `EndpointSpec` field for field. T-0249 replaces this file with the schemas the crate
 * itself exports; until the Portal can depend on it, a form needs a schema from somewhere.
 */

/** DNS-1123 label: what every manifest name and every space slug has to be (PF-09). */
export const DNS1123 = "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$";

/** RFC 4648 base32, lowercase, unpadded; 26 characters carry 130 bits of entropy (EP-02). */
export const SLUG_PATTERN = "^[a-z2-7]{26,}$";

/**
 * A manifest title is one plain string in the author's language (UI-50): one box, never a
 * language per input. A legacy map is collapsed by `plainTitle` before the form sees it.
 */
function titleProperty(label: string) {
  return { type: "string", title: label } as const;
}

export function contextSpaceSchema(t: (key: string) => string): JsonSchema {
  return {
    type: "object",
    required: ["name"],
    properties: {
      name: {
        type: "string",
        title: t("spaces.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: titleProperty(t("spaces.field.title")),
      dataModelRef: { type: "string", title: t("spaces.field.dataModel") },
      defaultLocale: {
        type: "string",
        title: t("spaces.field.locale"),
        enum: ["sk", "en", "de", "cs"],
      },
      isSandbox: { type: "boolean", title: t("spaces.field.sandbox"), default: false },
      ttlDays: {
        type: "integer",
        title: t("spaces.field.ttlDays"),
        minimum: 1,
        maximum: 14,
      },
    },
  };
}

export const REPRESENTATIONS = [
  "ngsi-ld",
  "geojson",
  "csv",
  "json",
  "xlsx",
  "zip",
  "ogc-features",
  "sta",
  "mcp",
] as const;

export const AUDIENCES = ["project-list", "organization", "public"] as const;

/**
 * The token-bucket classes a steward chooses between (EP-20).
 *
 * The contract field is a plain requests-per-minute number, and any number is valid on the
 * wire. Naming three of them is what turns a capacity decision into one somebody can make
 * without a calculator: strict for a hand-written client, standard for an application,
 * open for a scraper that pages through everything.
 */
export const RATE_LIMIT_CLASSES = { strict: 60, standard: 600, open: 6000 } as const;

/**
 * The schema formalisms an Endpoint publishes under `schema/v{major}/` (EP-46, EP-49).
 *
 * Every Endpoint serves all of them, so this is what to look at, never what to switch on.
 * The gateway compiles the first two itself and answers 406 for the rest until Model Tools
 * has committed them beside the source.
 */
export const SCHEMA_FORMALISMS = [
  "json-schema",
  "context.jsonld",
  "model.linkml.yaml",
  "model.ttl",
  "model.md",
] as const;

export type SchemaFormalism = (typeof SCHEMA_FORMALISMS)[number];

export function endpointSchema(
  t: (key: string) => string,
  spaces: string[],
  projects: string[] = [],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "audience", "enabledRepresentations"],
    properties: {
      name: {
        type: "string",
        title: t("endpoints.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: titleProperty(t("endpoints.field.title")),
      contextSpaceRef: {
        type: "string",
        title: t("endpoints.field.space"),
        ...(spaces.length > 0 ? { enum: spaces } : {}),
      },
      audience: {
        type: "string",
        title: t("endpoints.field.audience"),
        // The stored value stays the contract's word; the person reads what it means.
        oneOf: AUDIENCES.map((audience) => ({
          const: audience,
          title: t(`endpoints.audienceOption.${audience}`),
        })),
        default: "project-list",
      },
      enabledRepresentations: {
        type: "array",
        title: t("endpoints.field.representations"),
        items: {
          type: "string",
          oneOf: REPRESENTATIONS.map((representation) => ({
            const: representation,
            title: t(`endpoints.representationOption.${representation}`),
          })),
        },
        uniqueItems: true,
        minItems: 1,
      },
      // Required by the manifest when the audience is `project-list` and refused for the
      // other two (EP-14, EP-15). The page drops the property for the other two audiences and
      // prunes the value on the way out, because rjsf cannot both hide a field and keep the
      // value somebody already typed into it. With the project list at hand the field is a
      // set of checkboxes over the other projects of the repository, not free text (PF-05).
      allowedProjects: {
        type: "array",
        title: t("endpoints.field.allowedProjects"),
        items:
          projects.length > 0
            ? { type: "string", enum: projects }
            : { type: "string", pattern: DNS1123 },
        uniqueItems: true,
      },
      rateLimits: {
        type: "object",
        title: t("endpoints.field.rateLimits"),
        properties: {
          // EP-20 asks every endpoint to configure a limit, so the form carries one rather
          // than letting a steward publish an unlimited endpoint by leaving a field alone.
          // The Change shows it like any other field, so nothing lands unseen.
          requestsPerMinute: {
            type: "integer",
            title: t("endpoints.field.requestsPerMinute"),
            default: RATE_LIMIT_CLASSES.standard,
            oneOf: Object.entries(RATE_LIMIT_CLASSES).map(([name, perMinute]) => ({
              const: perMinute,
              title: `${t(`endpoints.rateClass.${name}`)} (${perMinute}/min)`,
            })),
          },
          burst: {
            type: "integer",
            title: t("endpoints.field.burst"),
            minimum: 1,
            maximum: 10000,
          },
        },
      },
      caching: {
        type: "object",
        title: t("endpoints.field.caching"),
        properties: {
          maxAgeSeconds: {
            type: "integer",
            title: t("endpoints.field.maxAge"),
            minimum: 0,
            maximum: 3600,
            multipleOf: 30,
          },
        },
      },
    },
  };
}

/** Checkboxes for the representation set, a slider for the cache TTL; the rest is default. */
export const endpointUiSchema: UiSchema = {
  enabledRepresentations: { "ui:widget": "checkboxes" },
  caching: { maxAgeSeconds: { "ui:widget": "range" } },
};

/**
 * 26 lowercase base32 characters from the platform CSPRNG: 130 bits, no space or project
 * name anywhere in it (EP-02, EP-03).
 */
export function generateSlug(): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz234567";
  const bytes = new Uint8Array(26);
  crypto.getRandomValues(bytes);
  // 256 is not a multiple of 32, so the last 8 values of a byte would be drawn slightly more
  // often; masking to 5 bits keeps every character equally likely.
  return Array.from(bytes, (byte) => alphabet[byte & 31]).join("");
}

/** The four feeds a `DataSource` connects to, in the order the wizard offers them (MF-35). */
export const DATA_SOURCE_TYPES = ["mqtt", "http", "websocket", "gtfs-rt"] as const;

export const TYPED_SOURCE_TYPES = DATA_SOURCE_TYPES;

export type TypedDataSourceType = (typeof DATA_SOURCE_TYPES)[number];

export type DataSourceType = TypedDataSourceType | string;

export function isTypedDataSource(type: string): type is TypedDataSourceType {
  return (DATA_SOURCE_TYPES as readonly string[]).includes(type);
}

/** The connection block each type carries, keyed the way the manifest keys it. */
export const CONNECTION_BLOCK: Record<TypedDataSourceType, string> = {
  mqtt: "mqtt",
  http: "http",
  websocket: "webSocket",
  "gtfs-rt": "gtfsRt",
};

/**
 * A credential reference, never a credential: the picker offers the secret names this project
 * already uses and takes a new one as free text, and the value itself is only ever in the
 * secret store (CC-06, MF-35).
 */
function secretRef(t: (key: string) => string, title: string, secrets: string[]): JsonSchema {
  return {
    type: "object",
    title,
    required: ["name", "key"],
    properties: {
      name: {
        type: "string",
        title: t("datasources.field.secretName"),
        ...(secrets.length > 0 ? { examples: secrets } : {}),
      },
      key: { type: "string", title: t("datasources.field.secretKey") },
    },
  };
}

/**
 * The form of one connection type (MF-35, UI-04).
 *
 * One schema per type rather than one schema with four optional blocks: the manifest allows
 * exactly the block its `type` names, and a form offering the other three invites a manifest
 * the API refuses.
 */
export function dataSourceSchema(
  t: (key: string) => string,
  type: TypedDataSourceType,
  secrets: string[] = [],
): JsonSchema {
  const name: JsonSchema = {
    type: "string",
    title: t("datasources.field.name"),
    pattern: DNS1123,
    maxLength: 63,
  };
  const title = titleProperty(t("datasources.field.title"));
  const tls: JsonSchema = {
    type: "object",
    title: t("datasources.field.tls"),
    properties: {
      caCertRef: secretRef(t, t("datasources.field.caCert"), secrets),
    },
  };

  const connection: Record<DataSourceType, JsonSchema> = {
    mqtt: {
      type: "object",
      title: t("datasources.type.mqtt"),
      required: ["urls", "topics"],
      properties: {
        urls: {
          type: "array",
          title: t("datasources.field.urls"),
          minItems: 1,
          items: { type: "string", pattern: "^(tcp|tls|ws|wss)://.+" },
        },
        topics: {
          type: "array",
          title: t("datasources.field.topics"),
          minItems: 1,
          items: { type: "string", minLength: 1 },
        },
        qos: { type: "integer", title: t("datasources.field.qos"), minimum: 0, maximum: 2 },
        cleanSession: { type: "boolean", title: t("datasources.field.cleanSession") },
        username: { type: "string", title: t("datasources.field.username") },
        passwordRef: secretRef(t, t("datasources.field.password"), secrets),
      },
    },
    http: {
      type: "object",
      title: t("datasources.type.http"),
      required: ["url"],
      properties: {
        url: { type: "string", title: t("datasources.field.url"), pattern: "^https?://.+" },
        verb: { type: "string", title: t("datasources.field.verb"), enum: ["GET", "POST"], default: "GET" },
        timeout: { type: "string", title: t("datasources.field.timeout"), pattern: "^[0-9]+(ms|s|m)$" },
        authorization: {
          type: "object",
          title: t("datasources.field.authorization"),
          properties: {
            // No default: a default would put an `authorization` block with a scheme and no
            // credential into every HTTP source, which the API refuses (`headerRef` missing).
            // The API writes `Bearer` in front of a credential when no scheme is named.
            scheme: { type: "string", title: t("datasources.field.scheme") },
            headerRef: secretRef(t, t("datasources.field.credential"), secrets),
          },
        },
      },
    },
    websocket: {
      type: "object",
      title: t("datasources.type.websocket"),
      required: ["url"],
      properties: {
        url: { type: "string", title: t("datasources.field.url"), pattern: "^wss?://.+" },
        openMessage: { type: "string", title: t("datasources.field.openMessage") },
      },
    },
    "gtfs-rt": {
      type: "object",
      title: t("datasources.type.gtfs-rt"),
      required: ["url", "feed"],
      properties: {
        url: { type: "string", title: t("datasources.field.url"), pattern: "^https?://.+" },
        feed: {
          type: "string",
          title: t("datasources.field.feed"),
          enum: ["vehiclePositions", "tripUpdates", "alerts"],
          default: "vehiclePositions",
        },
      },
    },
  };

  return {
    type: "object",
    required: ["name", CONNECTION_BLOCK[type]],
    properties: {
      name,
      title,
      [CONNECTION_BLOCK[type]]: connection[type],
      tls,
    },
  };
}

/** Long free text gets a text area; the rest is default rendering. */
export const dataSourceUiSchema: UiSchema = {
  webSocket: { openMessage: { "ui:widget": "textarea" } },
};

/** Field entry in the trimmed Bento inputs catalog (PL-50). */
export interface CatalogField {
  path: string;
  type: string;
  kind: string;
  secret: boolean;
  advanced: boolean;
  /** The runner documents the field as optional, whatever its default. */
  optional?: boolean;
  default: unknown;
  description: string;
}

/** Bento runner input catalog item (PL-50). */
export interface CatalogInput {
  name: string;
  group: string;
  summary: string;
  fields: CatalogField[];
}

/**
 * Generates JSON Schema and UiSchema from a runner input's field tree (PL-50).
 *
 * Nested dot paths become nested objects; arrays of objects and free-form objects edit as YAML;
 * secret fields use the `secretRef` widget; advanced fields are ordered last in `ui:order`.
 */
export function runnerInputSchema(input: CatalogInput): { schema: JsonSchema; uiSchema: UiSchema } {
  // ponytail: advanced: true fields are ordered last in ui:order and tagged with ui:options: { advanced: true }. A collapsible fold widget in SchemaForm is a follow-up.
  const schema: JsonSchema = {
    type: "object",
    properties: {},
    required: [],
  };
  const uiSchema: UiSchema = {};

  const containerPaths = new Set<string>();
  for (const f of input.fields) {
    const parts = f.path.split(".");
    for (let i = 1; i < parts.length; i++) {
      containerPaths.add(parts.slice(0, i).join("."));
    }
  }

  function getContainer(pathSegments: string[]): {
    objSchema: JsonSchema;
    objUi: Record<string, unknown>;
  } {
    let currSchema = schema;
    let currUi = uiSchema as Record<string, unknown>;

    for (const seg of pathSegments) {
      if (!currSchema.properties) {
        currSchema.properties = {};
      }
      if (!currSchema.properties[seg]) {
        currSchema.properties[seg] = {
          type: "object",
          properties: {},
          required: [],
        };
      }
      if (!currUi[seg]) {
        currUi[seg] = {};
      }
      currSchema = currSchema.properties[seg] as JsonSchema;
      currUi = currUi[seg] as Record<string, unknown>;
    }
    return { objSchema: currSchema, objUi: currUi };
  }

  for (const f of input.fields) {
    if (containerPaths.has(f.path)) {
      const parts = f.path.split(".");
      const { objSchema, objUi } = getContainer(parts);
      if (f.description) {
        objSchema.description = f.description;
      }
      if (f.advanced) {
        objUi["ui:options"] = {
          ...((objUi["ui:options"] as Record<string, unknown>) || {}),
          advanced: true,
        };
      }
      continue;
    }

    const parts = f.path.split(".");
    const leafKey = parts[parts.length - 1];
    const parentParts = parts.slice(0, -1);
    const { objSchema, objUi } = getContainer(parentParts);

    const propSchema: JsonSchema = {};
    const propUi: Record<string, unknown> = {};

    if (f.secret) {
      propSchema.type = "string";
      propUi["ui:widget"] = "secretRef";
    } else if (f.kind === "array") {
      if (f.type === "string") {
        propSchema.type = "array";
        propSchema.items = { type: "string" };
      } else if (f.type === "int") {
        propSchema.type = "array";
        propSchema.items = { type: "integer" };
      } else if (f.type === "float") {
        propSchema.type = "array";
        propSchema.items = { type: "number" };
      } else if (f.type === "bool") {
        propSchema.type = "array";
        propSchema.items = { type: "boolean" };
      } else {
        propSchema.type = "string";
        propUi["ui:widget"] = "textarea";
      }
    } else if (f.kind === "scalar") {
      if (f.type === "string") {
        propSchema.type = "string";
      } else if (f.type === "int") {
        propSchema.type = "integer";
      } else if (f.type === "float") {
        propSchema.type = "number";
      } else if (f.type === "bool") {
        propSchema.type = "boolean";
      } else {
        propSchema.type = "string";
        propUi["ui:widget"] = "textarea";
      }
    } else {
      propSchema.type = "string";
      propUi["ui:widget"] = "textarea";
    }

    // A YAML-edited field is a string in the form: a list or map default stays the runner's own
    // and is not sent, since `[]` in a string field fails the form's validation.
    const yamlDefault = propSchema.type === "string" && typeof f.default !== "string";
    if (f.default !== null && f.default !== undefined && !yamlDefault) {
      propSchema.default = f.default as JsonSchema["default"];
    }
    if (f.description) {
      propSchema.description = f.description;
    }

    if (f.advanced) {
      propUi["ui:options"] = {
        ...((propUi["ui:options"] as Record<string, unknown>) || {}),
        advanced: true,
      };
    }

    if (!objSchema.properties) {
      objSchema.properties = {};
    }
    objSchema.properties[leafKey] = propSchema;

    if (Object.keys(propUi).length > 0) {
      objUi[leafKey] = propUi;
    }

    // Required is what the runner requires: no default, not advanced, not marked optional.
    if ((f.default === null || f.default === undefined) && !f.advanced && !f.optional) {
      if (!objSchema.required) {
        objSchema.required = [];
      }
      objSchema.required.push(leafKey);
    }
  }

  function applyOrder(
    props: Record<string, JsonSchema> | undefined,
    targetUi: Record<string, unknown>,
    prefix: string
  ) {
    if (!props) return;
    const keys = Object.keys(props);
    if (keys.length === 0) return;

    const nonAdvanced: string[] = [];
    const advanced: string[] = [];

    for (const k of keys) {
      const fieldPath = prefix ? `${prefix}.${k}` : k;
      const f = input.fields.find((field) => field.path === fieldPath);
      const isAdv =
        f?.advanced ||
        Boolean(
          (targetUi[k] as Record<string, unknown> | undefined)?.["ui:options"] &&
            ((targetUi[k] as Record<string, unknown>)["ui:options"] as Record<string, unknown>)
              .advanced
        );
      if (isAdv) {
        advanced.push(k);
      } else {
        nonAdvanced.push(k);
      }

      const childSchema = props[k];
      if (
        childSchema &&
        typeof childSchema === "object" &&
        childSchema.type === "object" &&
        childSchema.properties
      ) {
        targetUi[k] = targetUi[k] || {};
        applyOrder(
          childSchema.properties as Record<string, JsonSchema>,
          targetUi[k] as Record<string, unknown>,
          fieldPath
        );
      }
    }

    targetUi["ui:order"] = [...nonAdvanced, ...advanced, "*"];
  }

  applyOrder(schema.properties as Record<string, JsonSchema>, uiSchema as Record<string, unknown>, "");

  return { schema, uiSchema };
}

/** Prepares envelope-level schema for a runner input by prepending `name` and `title`. */
export function runnerDataSourceSchema(
  t: (key: string) => string,
  input: CatalogInput
): { schema: JsonSchema; uiSchema: UiSchema } {
  const { schema: inputSchema, uiSchema: inputUiSchema } = runnerInputSchema(input);

  const name: JsonSchema = {
    type: "string",
    title: t("datasources.field.name"),
    pattern: DNS1123,
    maxLength: 63,
  };
  const title = titleProperty(t("datasources.field.title"));

  const schema: JsonSchema = {
    type: "object",
    required: ["name", ...(inputSchema.required ?? [])],
    properties: {
      name,
      title,
      ...(inputSchema.properties ?? {}),
    },
  };

  const inputOrder = (inputUiSchema["ui:order"] as string[]) ?? [];
  const uiOrder = ["name", "title", ...inputOrder.filter((k) => k !== "*"), "*"];

  const uiSchema: UiSchema = {
    ...inputUiSchema,
    "ui:order": uiOrder,
  };

  return { schema, uiSchema };
}

/** The field paths a runner input documents as YAML documents (objects, maps, lists of objects). */
export function yamlPathsOf(inputDef?: CatalogInput): Set<string> {
  const paths = new Set<string>();
  for (const f of inputDef?.fields ?? []) {
    const yamlType = f.type === "object" || f.type === "unknown";
    if (f.kind === "map" || ((f.kind === "scalar" || f.kind === "array") && yamlType)) {
      paths.add(f.path);
    }
  }
  return paths;
}

/** A YAML-edited field the form cannot serialise; `field` is the path the runner documents. */
export class YamlFieldError extends Error {
  constructor(
    public readonly field: string,
    public readonly detail: string
  ) {
    super(`${field}: not valid YAML: ${detail}`);
  }
}

/**
 * Turns the YAML text a textarea holds back into the object or list the manifest carries.
 * A field the catalog marks as YAML is always parsed; any other string is parsed only when it
 * looks like a document (a newline, or a leading `-`, `{`, `[` or `:`). A parse error throws
 * `YamlFieldError`, so a broken value never reaches the API as a string.
 */
export function parseYamlStrings(val: unknown, yamlPaths: Set<string> = new Set(), path = ""): unknown {
  if (typeof val === "string") {
    const trimmed = val.trim();
    if (yamlPaths.has(path) || /^[-{[:]/.test(trimmed) || trimmed.includes("\n")) {
      try {
        const parsed = parseYaml(val);
        return typeof parsed === "object" && parsed !== null ? parsed : val;
      } catch (err) {
        throw new YamlFieldError(path, err instanceof Error ? err.message : String(err));
      }
    }
    return val;
  }
  if (Array.isArray(val)) {
    return val.map((v) => parseYamlStrings(v, yamlPaths, path));
  }
  if (val && typeof val === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(val as Record<string, unknown>)) {
      out[k] = parseYamlStrings(v, yamlPaths, path ? `${path}.${k}` : k);
    }
    return out;
  }
  return val;
}

/** Finds all `${VAR}` interpolation names across an object. */
export function findEnvVars(obj: unknown, found: Set<string>): void {
  if (typeof obj === "string") {
    const matches = obj.matchAll(/\$\{([A-Z0-9_]+)\}/g);
    for (const m of matches) {
      found.add(m[1]);
    }
  } else if (Array.isArray(obj)) {
    for (const item of obj) {
      findEnvVars(item, found);
    }
  } else if (obj && typeof obj === "object") {
    for (const val of Object.values(obj)) {
      findEnvVars(val, found);
    }
  }
}

/** The execution classes of a Pipeline, `auto` letting the reconciler pick (PL-04, PL-26). */
export const PIPELINE_CLASSES = ["auto", "resident", "scheduled"] as const;

/** The compute engines a pipeline step may run on, lightest first (PL-33). */
export const COMPUTE_KINDS = ["bloblang", "mapping", "wasm", "container"] as const;

/** How a derived pipeline writes its result (PL-32). */
export const OUTPUT_MODES = ["upsert", "update-attrs"] as const;

/** A Bento duration: a positive count and one of the units Bento accepts (PL-26, PL-27). */
export const PERIOD_PATTERN = "^[1-9][0-9]*(ms|s|m|h)$";

/** Five whitespace-separated fields; the CronJob controller parses the rest (PL-04). */
export const CRON_PATTERN = "^\\S+\\s+\\S+\\s+\\S+\\s+\\S+\\s+\\S+$";

/** An NGSI-LD entity type short name, PascalCase, 2 to 64 characters. */
export const ENTITY_TYPE_PATTERN = "^[A-Z][A-Za-z0-9]{1,63}$";

/** The URN of an Endpoint: `urn:ngsi-ld:Endpoint:{orgDomain}:{space}:{name}` (PF-39, PL-18). */
export const ENDPOINT_URN_PATTERN = "^urn:ngsi-ld:Endpoint:[^:]+:[^:]+:[^:]+$";

/** One Endpoint the pipeline form may pick, by name as a source and by URN as a target. */
export interface EndpointOption {
  name: string;
  urn?: string;
}

/** A select when the project has something to offer, free text with the pattern otherwise. */
function choice(base: JsonSchema, values: string[]): JsonSchema {
  return values.length > 0 ? { ...base, enum: values } : base;
}

/**
 * `PipelineSpec`, field for field, with the rules `PipelineSpec::validate` checks written as
 * conditions so the form refuses what the reconciler would refuse (PL-04, PL-31, PL-33, PL-39).
 *
 * The mapping of a `bloblang` step is `compute.bloblang` (PL-41), rendered as the last
 * processor of the stream; left empty, the author keeps it in `bento.yaml` beside the manifest.
 */
export function pipelineSchema(
  t: (key: string) => string,
  dataSources: string[],
  endpoints: EndpointOption[],
): JsonSchema {
  const entityType = (title: string): JsonSchema => ({
    type: "string",
    title,
    pattern: ENTITY_TYPE_PATTERN,
  });
  const names = (title: string): JsonSchema => ({
    type: "array",
    title,
    items: { type: "string", minLength: 1 },
  });
  // The option a person picks is the endpoint's name; the value the manifest carries is the
  // URN the organization domain completes (PL-04).
  const targets = endpoints.flatMap((endpoint) =>
    endpoint.urn ? [{ const: endpoint.urn, title: endpoint.name }] : [],
  );

  return {
    type: "object",
    required: ["name", "class", "targetEndpoint"],
    properties: {
      name: {
        type: "string",
        title: t("pipelines.field.id"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: titleProperty(t("pipelines.field.title")),
      class: {
        type: "string",
        title: t("pipelines.field.class"),
        description: t("pipelines.field.classHint"),
        enum: [...PIPELINE_CLASSES],
        default: "auto",
      },
      schedule: {
        type: "string",
        title: t("pipelines.field.schedule"),
        description: t("pipelines.field.scheduleHint"),
        pattern: CRON_PATTERN,
      },
      period: {
        type: "string",
        title: t("pipelines.field.period"),
        description: t("pipelines.field.periodHint"),
        pattern: PERIOD_PATTERN,
      },
      source: {
        type: "object",
        title: t("pipelines.field.source"),
        description: t("pipelines.field.sourceHint"),
        properties: {
          dataSourceRef: choice(
            { type: "string", title: t("pipelines.field.dataSource"), pattern: DNS1123 },
            dataSources,
          ),
          endpointRef: choice(
            { type: "string", title: t("pipelines.field.sourceEndpoint"), pattern: DNS1123 },
            endpoints.map((endpoint) => endpoint.name),
          ),
          query: {
            type: "object",
            title: t("pipelines.field.query"),
            properties: {
              type: entityType(t("pipelines.field.queryType")),
              attrs: names(t("pipelines.field.attrs")),
              // The ticked entities of the studio's sample (PL-42): PF-42 URNs.
              ids: {
                type: "array",
                title: t("pipelines.field.ids"),
                description: t("pipelines.field.idsHint"),
                items: { type: "string", pattern: "^urn:ngsi-ld:[^:]+:[^:]+:[^:]+:[A-Za-z0-9._~-]{1,128}$" },
              },
              q: { type: "string", title: t("pipelines.field.q") },
              scopeQ: { type: "string", title: t("pipelines.field.scopeQ") },
              geoQ: { type: "string", title: t("pipelines.field.geoQ") },
              temporalQ: {
                type: "object",
                title: t("pipelines.field.temporalQ"),
                properties: {
                  window: {
                    type: "string",
                    title: t("pipelines.field.temporalWindow"),
                    pattern: "^P",
                  },
                },
              },
            },
          },
          trigger: {
            type: "object",
            title: t("pipelines.field.trigger"),
            properties: {
              subscription: {
                type: "object",
                title: t("pipelines.field.subscription"),
                properties: {
                  type: entityType(t("pipelines.field.triggerType")),
                  watchedAttributes: names(t("pipelines.field.watchedAttributes")),
                },
                // A subscription is its type; attributes alone name nothing to watch.
                dependencies: { watchedAttributes: ["type"] },
              },
            },
          },
        },
        // One input: the outside world through a DataSource or the platform's own spaces
        // through an Endpoint, never both (PL-39).
        not: { required: ["dataSourceRef", "endpointRef"] },
      },
      compute: {
        type: "object",
        title: t("pipelines.field.compute"),
        description: t("pipelines.field.computeHint"),
        properties: {
          kind: {
            type: "string",
            title: t("pipelines.field.computeKind"),
            enum: [...COMPUTE_KINDS],
          },
          module: { type: "string", title: t("pipelines.field.module") },
          function: { type: "string", title: t("pipelines.field.function") },
          mappingRef: { type: "string", title: t("pipelines.field.mappingRef"), pattern: DNS1123 },
          bloblang: {
            type: "string",
            title: t("pipelines.field.bloblang"),
            description: t("pipelines.field.bloblangHint"),
          },
        },
        dependencies: { module: ["kind"], function: ["kind"], mappingRef: ["kind"], bloblang: ["kind"] },
        allOf: [
          {
            // The inline mapping belongs to a bloblang step only (PL-41).
            if: { required: ["bloblang"] },
            then: { properties: { kind: { const: "bloblang" } }, required: ["kind"] },
          },
          {
            if: { properties: { kind: { const: "wasm" } }, required: ["kind"] },
            then: { required: ["module", "function"] },
          },
          {
            if: { properties: { kind: { const: "mapping" } }, required: ["kind"] },
            then: {
              required: ["mappingRef"],
              not: { anyOf: [{ required: ["module"] }, { required: ["function"] }] },
            },
          },
        ],
      },
      targetEndpoint: {
        type: "string",
        title: t("pipelines.field.targetEndpoint"),
        description: t("pipelines.field.targetEndpointHint"),
        ...(targets.length > 0 ? { oneOf: targets } : { pattern: ENDPOINT_URN_PATTERN }),
      },
      output: {
        type: "object",
        title: t("pipelines.field.output"),
        properties: {
          type: entityType(t("pipelines.field.outputType")),
          mode: {
            type: "string",
            title: t("pipelines.field.outputMode"),
            enum: [...OUTPUT_MODES],
          },
        },
        // The manifest takes both or neither.
        dependencies: { type: ["mode"], mode: ["type"] },
      },
      allowFeedback: {
        type: "boolean",
        title: t("pipelines.field.allowFeedback"),
        description: t("pipelines.field.allowFeedbackHint"),
      },
      secretRefs: {
        type: "array",
        title: t("pipelines.field.secrets"),
        description: t("pipelines.field.secretsHint"),
        items: {
          type: "object",
          required: ["name"],
          properties: {
            name: { type: "string", title: t("pipelines.field.secretName"), pattern: DNS1123 },
            key: { type: "string", title: t("pipelines.field.secretKey") },
            envVar: { type: "string", title: t("pipelines.field.envVar"), pattern: "^[A-Z_][A-Z0-9_]*$" },
          },
        },
      },
      quotas: {
        type: "object",
        title: t("pipelines.field.quotas"),
        properties: {
          maxMemoryMb: { type: "integer", title: t("pipelines.field.maxMemoryMb"), minimum: 1 },
          cpuMillicores: { type: "integer", title: t("pipelines.field.cpuMillicores"), minimum: 1 },
        },
      },
    },
    allOf: [
      {
        if: { properties: { class: { const: "scheduled" } }, required: ["class"] },
        then: { required: ["schedule"] },
      },
      {
        if: { properties: { class: { const: "resident" } }, required: ["class"] },
        then: { not: { required: ["schedule"] } },
      },
    ],
  };
}

/** The manifest's own order, so the form reads like the YAML it writes. */
export const pipelineUiSchema: UiSchema = {
  "ui:order": [
    "name",
    "title",
    "class",
    "schedule",
    "period",
    "source",
    "compute",
    "targetEndpoint",
    "output",
    "allowFeedback",
    "secretRefs",
    "quotas",
    "*",
  ],
  source: { "ui:order": ["dataSourceRef", "endpointRef", "query", "trigger", "*"] },
  compute: {
    "ui:order": ["kind", "bloblang", "mappingRef", "module", "function", "*"],
    bloblang: { "ui:widget": "textarea", "ui:options": { rows: 14 } },
  },
};

// ---------------------------------------------------------------------------------------------
// Dashboards and layers (T-0528, UI-17, UI-18): `DashboardSpec` and `LayerSpec` of jc-core.

export const DASHBOARD_VISIBILITIES = ["private", "project", "organization", "public"] as const;
export const PAGE_LAYOUTS = ["full-map", "grid-2x2"] as const;
export const LAYER_STYLES = ["circle", "line", "fill", "heatmap", "hexagon", "icon"] as const;

const numberPair = (title: string): JsonSchema => ({
  type: "array",
  title,
  items: { type: "number" },
  minItems: 2,
  maxItems: 2,
});

export function dashboardSchema(t: (key: string) => string, layers: string[]): JsonSchema {
  return {
    type: "object",
    required: ["name", "title", "visibility", "pages"],
    properties: {
      name: { type: "string", title: t("dashboards.field.name"), pattern: DNS1123, maxLength: 63 },
      title: titleProperty(t("dashboards.field.title")),
      visibility: {
        type: "string",
        title: t("dashboards.field.visibility"),
        enum: [...DASHBOARD_VISIBILITIES],
        default: "project",
      },
      pages: {
        type: "array",
        title: t("dashboards.field.pages"),
        minItems: 1,
        items: {
          type: "object",
          properties: {
            title: { type: "string", title: t("dashboards.field.pageTitle") },
            layout: {
              type: "string",
              title: t("dashboards.field.layout"),
              enum: [...PAGE_LAYOUTS],
              default: "full-map",
            },
            layers: {
              type: "array",
              title: t("dashboards.field.layers"),
              items: layers.length > 0 ? { type: "string", enum: layers } : { type: "string", pattern: DNS1123 },
              uniqueItems: true,
            },
            widgets: {
              type: "array",
              title: t("dashboards.field.widgets"),
              items: {
                type: "object",
                required: ["widgetType"],
                properties: {
                  widgetType: { type: "string", title: t("dashboards.field.widgetType") },
                  endpointRef: { type: "string", title: t("dashboards.field.endpoint"), pattern: DNS1123 },
                  entityId: { type: "string", title: t("dashboards.field.entityId") },
                  property: { type: "string", title: t("dashboards.field.property") },
                },
              },
            },
          },
        },
      },
    },
  };
}

export const dashboardUiSchema: UiSchema = {
  pages: { items: { layers: { "ui:widget": "checkboxes" } } },
};

export function layerSchema(
  t: (key: string) => string,
  endpoints: string[],
  types: string[],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "sourceEndpointRef", "entityType", "style"],
    properties: {
      name: { type: "string", title: t("dashboards.field.name"), pattern: DNS1123, maxLength: 63 },
      sourceEndpointRef: {
        type: "string",
        title: t("dashboards.field.endpoint"),
        ...(endpoints.length > 0 ? { enum: endpoints } : { pattern: DNS1123 }),
      },
      entityType: {
        type: "string",
        title: t("dashboards.field.entityType"),
        ...(types.length > 0 ? { enum: types } : { pattern: ENTITY_TYPE_PATTERN }),
      },
      style: { type: "string", title: t("dashboards.field.style"), enum: [...LAYER_STYLES], default: "circle" },
      visible: { type: "boolean", title: t("dashboards.field.visible"), default: true },
      filter: {
        type: "object",
        title: t("dashboards.field.filter"),
        properties: {
          q: { type: "string", title: "q" },
          scopeQ: { type: "string", title: "scopeQ" },
          geoQ: { type: "string", title: "geoQ" },
        },
      },
      colorBy: {
        type: "object",
        title: t("dashboards.field.colorBy"),
        properties: {
          property: { type: "string", title: t("dashboards.field.property") },
          palette: { type: "string", title: t("dashboards.field.palette") },
          domain: numberPair(t("dashboards.field.domain")),
        },
      },
      sizeBy: {
        type: "object",
        title: t("dashboards.field.sizeBy"),
        properties: {
          property: { type: "string", title: t("dashboards.field.property") },
          range: numberPair(t("dashboards.field.range")),
        },
      },
      popupProperties: {
        type: "array",
        title: t("dashboards.field.popup"),
        items: { type: "string" },
        uniqueItems: true,
      },
    },
  };
}

export const layerUiSchema: UiSchema = {
  filter: { q: { "ui:autocomplete": "off" } },
};
