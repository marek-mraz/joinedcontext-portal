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
    },
  };
}

/** Checkboxes for the representation set; everything else takes the default widget. */
export const endpointUiSchema: UiSchema = {
  enabledRepresentations: { "ui:widget": "checkboxes" },
  slug: { "ui:autocomplete": "off" },
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
