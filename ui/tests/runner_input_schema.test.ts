import { describe, expect, it } from "vitest";
import { runnerInputSchema } from "../src/schemas/kinds";
import type { CatalogInput } from "../src/schemas/kinds";
import { toEnvelope, toForm } from "../src/pages/datasources/DataSourcesPage";
import type { Manifest } from "../src/api/manifest";

const KAFKA_CATALOG_ENTRY: CatalogInput = {
  name: "kafka",
  group: "brokers",
  summary: "Connects to Kafka brokers and consumes one or more topics.",
  fields: [
    {
      path: "addresses",
      type: "string",
      kind: "array",
      secret: false,
      advanced: false,
      default: null,
      description: "A list of broker addresses to connect to.",
    },
    {
      path: "topics",
      type: "string",
      kind: "array",
      secret: false,
      advanced: false,
      default: null,
      description: "A list of topics to consume from.",
    },
    {
      path: "consumer_group",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: false,
      default: "",
      description: "An identifier for the consumer group.",
    },
    {
      path: "tls",
      type: "object",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: null,
      description: "Custom TLS settings.",
    },
    {
      path: "tls.enabled",
      type: "bool",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: false,
      description: "Whether custom TLS settings are enabled.",
    },
    {
      path: "sasl",
      type: "object",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: null,
      description: "Enables SASL authentication.",
    },
    {
      path: "sasl.mechanism",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: "none",
      description: "The SASL authentication mechanism.",
    },
    {
      path: "sasl.user",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: "",
      description: "A PLAIN username.",
    },
    {
      path: "sasl.password",
      type: "string",
      kind: "scalar",
      secret: true,
      advanced: true,
      default: "",
      description: "A PLAIN password.",
    },
  ],
};

describe("runnerInputSchema pure function", () => {
  it("generates JSON Schema and UiSchema with required non-advanced fields and secretRef widget", () => {
    const { schema, uiSchema } = runnerInputSchema(KAFKA_CATALOG_ENTRY);

    expect(schema.type).toBe("object");
    const props = schema.properties as Record<string, { type?: string; items?: { type?: string }; properties?: Record<string, unknown> }>;
    expect(props.addresses).toBeDefined();
    expect(props.addresses.type).toBe("array");
    expect(props.addresses.items?.type).toBe("string");

    // addresses has no default and advanced=false -> required
    expect(schema.required).toContain("addresses");

    // sasl is a nested object
    expect(props.sasl).toBeDefined();
    expect(props.sasl.type).toBe("object");
    expect(props.sasl.properties).toBeDefined();
    expect((props.sasl.properties as Record<string, { type?: string }>).password?.type).toBe("string");

    // secret field gets secretRef widget
    const saslUi = uiSchema.sasl as Record<string, Record<string, unknown>> | undefined;
    expect(saslUi?.password?.["ui:widget"]).toBe("secretRef");

    // advanced fields are ordered last in ui:order
    const order = (uiSchema["ui:order"] as string[]) ?? [];
    expect(order).toContain("addresses");
    expect(order).toContain("topics");
    expect(order).toContain("consumer_group");
    expect(order).toContain("tls");
    expect(order).toContain("sasl");

    const nonAdvancedIndices = ["addresses", "topics", "consumer_group"].map((f) => order.indexOf(f));
    const advancedIndices = ["tls", "sasl"].map((f) => order.indexOf(f));
    for (const nonAdv of nonAdvancedIndices) {
      for (const adv of advancedIndices) {
        expect(nonAdv).toBeLessThan(adv);
      }
    }
  });

  it("toEnvelope builds spec.type, nested input, and retains only referenced secrets", () => {
    const form = {
      name: "kafka-helsinki",
      title: { en: "Helsinki Air Feed" },
      addresses: ["kafka.hel.fi:9093"],
      topics: ["sensors.air"],
      sasl: {
        mechanism: "SCRAM-SHA-512",
        user: "hki-collector",
        password: "${DS_KAFKA_HEL_PASSWORD}",
      },
      secrets: [
        { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
        { name: "unused-secret", key: "key", envVar: "DS_UNUSED_SECRET" },
      ],
    };

    const envelope = toEnvelope("helsinki", "kafka", form);

    expect(envelope.apiVersion).toBe("joinedcontext.com/v1alpha1");
    expect(envelope.kind).toBe("DataSource");
    expect(envelope.metadata.name).toBe("kafka-helsinki");
    expect(envelope.metadata.namespace).toBe("helsinki");
    expect(envelope.spec.type).toBe("kafka");

    const input = envelope.spec.input as Record<string, unknown>;
    expect(input.addresses).toEqual(["kafka.hel.fi:9093"]);
    expect(input.topics).toEqual(["sensors.air"]);
    expect((input.sasl as Record<string, unknown>).password).toBe("${DS_KAFKA_HEL_PASSWORD}");
    expect(input).not.toHaveProperty("name");
    expect(input).not.toHaveProperty("title");
    expect(input).not.toHaveProperty("secrets");

    // Only secrets whose envVar is used in input are kept
    expect(envelope.spec.secrets).toEqual([
      { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
    ]);
  });

  it("toForm round-trips a stored runner manifest", () => {
    const stored: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: {
        name: "kafka-live",
        namespace: "helsinki",
        title: { en: "Live Kafka Feed" },
      },
      spec: {
        type: "kafka",
        input: {
          addresses: ["kafka.hel.fi:9093"],
          topics: ["sensors.air"],
          sasl: {
            mechanism: "SCRAM-SHA-512",
            user: "hki-collector",
            password: "${DS_KAFKA_HEL_PASSWORD}",
          },
        },
        secrets: [
          { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
        ],
      },
    };

    const form = toForm(stored);
    expect(form.name).toBe("kafka-live");
    expect(form.title).toEqual({ en: "Live Kafka Feed" });
    expect(form.addresses).toEqual(["kafka.hel.fi:9093"]);
    expect(form.topics).toEqual(["sensors.air"]);
    expect((form.sasl as Record<string, unknown>).password).toBe("${DS_KAFKA_HEL_PASSWORD}");
    expect(form.secrets).toEqual([
      { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
    ]);

    const roundTripped = toEnvelope("helsinki", "kafka", form);
    expect(roundTripped.spec.type).toBe("kafka");
    expect(roundTripped.spec.input).toEqual(stored.spec.input);
    expect(roundTripped.spec.secrets).toEqual(stored.spec.secrets);
  });
});
