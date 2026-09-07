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

const titleProperty = {
  type: "object",
  properties: {
    sk: { type: "string", title: "sk" },
    en: { type: "string", title: "en" },
    de: { type: "string", title: "de" },
    cs: { type: "string", title: "cs" },
  },
  additionalProperties: { type: "string" },
} as const;

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
      title: { ...titleProperty, title: t("spaces.field.title") },
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
    required: ["name", "slug", "contextSpaceRef", "audience", "enabledRepresentations"],
    properties: {
      name: {
        type: "string",
        title: t("endpoints.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: { ...titleProperty, title: t("endpoints.field.title") },
      contextSpaceRef: {
        type: "string",
        title: t("endpoints.field.space"),
        ...(spaces.length > 0 ? { enum: spaces } : {}),
      },
      slug: {
        type: "string",
        title: t("endpoints.field.slug"),
        pattern: SLUG_PATTERN,
      },
      audience: {
        type: "string",
        title: t("endpoints.field.audience"),
        enum: [...AUDIENCES],
        default: "project-list",
      },
      enabledRepresentations: {
        type: "array",
        title: t("endpoints.field.representations"),
        items: { type: "string", enum: [...REPRESENTATIONS] },
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
  slug: { "ui:autocomplete": "off" },
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

export type DataSourceType = (typeof DATA_SOURCE_TYPES)[number];

/** The connection block each type carries, keyed the way the manifest keys it. */
export const CONNECTION_BLOCK: Record<DataSourceType, string> = {
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
  type: DataSourceType,
  secrets: string[] = [],
): JsonSchema {
  const name: JsonSchema = {
    type: "string",
    title: t("datasources.field.name"),
    pattern: DNS1123,
    maxLength: 63,
  };
  const title = { ...titleProperty, title: t("datasources.field.title") };
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
            scheme: { type: "string", title: t("datasources.field.scheme"), default: "Bearer" },
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

/** How a registration's answer relates to the broker's own data (CIM 009 clause 5.2.9). */
export const REGISTRATION_MODES = ["inclusive", "exclusive", "auxiliary", "redirect"] as const;

export type RegistrationMode = (typeof REGISTRATION_MODES)[number];

/** Which identity a forwarded request carries (PF-48). */
export const FEDERATION_IDENTITIES = ["serviceAccount", "caller"] as const;

/**
 * `ContextSourceRegistrationSpec`, field for field (MF-36).
 *
 * The two targets are separate fields rather than one, because that is what the manifest has
 * and a form that merged them would have to guess which the author meant. jc-core refuses a
 * registration naming both, so the hint says so where the author is typing.
 */
export function contextSourceRegistrationSchema(t: (key: string) => string): JsonSchema {
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "information"],
    properties: {
      name: {
        type: "string",
        title: t("federation.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: { ...titleProperty, title: t("federation.field.title") },
      contextSpaceRef: {
        type: "string",
        title: t("federation.field.space"),
        description: t("federation.field.spaceHint"),
        pattern: DNS1123,
      },
      endpointRef: {
        type: "object",
        title: t("federation.field.endpointRef"),
        description: t("federation.field.targetHint"),
        properties: {
          kind: { type: "string", default: "Endpoint", readOnly: true },
          name: { type: "string", title: t("federation.field.name"), pattern: DNS1123 },
        },
      },
      endpoint: {
        type: "string",
        title: t("federation.field.endpoint"),
        description: t("federation.field.targetHint"),
        pattern: "^https?://.+",
      },
      information: {
        type: "array",
        title: t("federation.field.information"),
        description: t("federation.field.informationHint"),
        minItems: 1,
        // One empty claim to start with. The field is required, so a form that opened with an
        // empty array would show the author an "Add" button and no reason to press it.
        default: [{ entities: [{}] }],
        items: {
          type: "object",
          required: ["entities"],
          properties: {
            entities: {
              type: "array",
              title: t("federation.field.entities"),
              minItems: 1,
              items: {
                type: "object",
                required: ["type"],
                properties: {
                  type: { type: "string", title: t("federation.field.entityType") },
                  idPattern: { type: "string", title: t("federation.field.idPattern") },
                },
              },
            },
            propertyNames: {
              type: "array",
              title: t("federation.field.propertyNames"),
              items: { type: "string" },
            },
          },
        },
      },
      mode: {
        type: "string",
        title: t("federation.field.mode"),
        enum: [...REGISTRATION_MODES],
        default: "inclusive",
      },
      operations: {
        type: "array",
        title: t("federation.field.operations"),
        description: t("federation.field.operationsHint"),
        items: { type: "string", minLength: 1 },
      },
      federation: {
        type: "object",
        title: t("federation.field.federation"),
        properties: {
          identity: {
            type: "string",
            title: t("federation.field.identity"),
            description: t("federation.field.identityHint"),
            enum: [...FEDERATION_IDENTITIES],
            default: "serviceAccount",
          },
          serviceAccountRef: {
            type: "object",
            title: t("federation.field.serviceAccount"),
            properties: {
              kind: { type: "string", default: "ServiceAccount", readOnly: true },
              name: { type: "string", title: t("federation.field.name"), pattern: DNS1123 },
            },
          },
        },
      },
    },
  };
}

export const contextSourceRegistrationUiSchema: UiSchema = {
  "ui:order": [
    "name",
    "title",
    "contextSpaceRef",
    "endpointRef",
    "endpoint",
    "information",
    "mode",
    "operations",
    "federation",
    "*",
  ],
};

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
  const targets = endpoints.flatMap((endpoint) => (endpoint.urn ? [endpoint.urn] : []));

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
      title: { ...titleProperty, title: t("pipelines.field.title") },
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
      targetEndpoint: choice(
        {
          type: "string",
          title: t("pipelines.field.targetEndpoint"),
          description: t("pipelines.field.targetEndpointHint"),
          pattern: ENDPOINT_URN_PATTERN,
        },
        targets,
      ),
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
      title: { ...titleProperty, title: t("dashboards.field.title") },
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
