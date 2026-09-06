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
      // other two (EP-14, EP-15). The page prunes it rather than the schema, because rjsf
      // cannot both hide a field and keep the value somebody already typed into it.
      allowedProjects: {
        type: "array",
        title: t("endpoints.field.allowedProjects"),
        items: { type: "string", pattern: DNS1123 },
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
export const DATA_SOURCE_TYPES = ["mqtt", "http", "web-socket", "gtfs-rt"] as const;

export type DataSourceType = (typeof DATA_SOURCE_TYPES)[number];

/** The connection block each type carries, keyed the way the manifest keys it. */
export const CONNECTION_BLOCK: Record<DataSourceType, string> = {
  mqtt: "mqtt",
  http: "http",
  "web-socket": "webSocket",
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
    "web-socket": {
      type: "object",
      title: t("datasources.type.web-socket"),
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
