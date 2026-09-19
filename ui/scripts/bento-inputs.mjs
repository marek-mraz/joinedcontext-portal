#!/usr/bin/env node
// The runner's input catalog, read from the pinned image and never from a manual (PL-50):
//   docker run --rm ghcr.io/warpstreamlabs/bento:1.21.1 list --format json-full \
//     | node scripts/bento-inputs.mjs src/schemas/bento-inputs.json \
//         ../../joinedcontext-platform/crates/jc-core/src/kinds/bento_inputs.rs
// Writes the trimmed catalog the Data Sources form is generated from: per input its name,
// the runner's one-line summary, the operator group and the field tree (path, type, kind,
// secret, advanced, optional, default, first sentence of the description). Nested objects are walked;
// arrays of objects are kept as one field the form edits as YAML.
import { readFileSync, writeFileSync } from "node:fs";

// Fields the runner's own documentation does not mark `is_secret` and that carry a credential all
// the same (T-2239, MF-24). Matched by path suffix, so one entry covers every input that has the
// field. A field this list misses renders as a plain text box in the Data Sources form and escapes
// the `${VAR}` rule, which is how an OAuth bearer reached a manifest and Git.
const ALSO_SECRET = [
  "oauth.access_token", // the bearer itself (http_client, websocket)
  "digest_auth.password", // a password (http_client)
  "sasl.access_token", // the bearer itself (kafka)
  "credentials.token", // an AWS session token (aws_*, sql_*, kafka's sasl.aws)
  "credentials.id", // an AWS access key id: the other half of a credential pair
  "auth.token.token", // pulsar's token
  "api_key", // twitter_search
];

/** Whether a field path is one this platform calls a secret although the runner does not. */
function alsoSecret(path) {
  return ALSO_SECRET.some((suffix) => path === suffix || path.endsWith(`.${suffix}`));
}

const GROUPS = {
  brokers: ["amqp_0_9", "amqp_1", "beanstalkd", "kafka", "kafka_franz", "mqtt", "nanomsg", "nats", "nats_jetstream", "nats_kv", "nats_object_store", "nats_stream", "nsq", "pulsar", "redis_list", "redis_pubsub", "redis_streams", "zmq4n"],
  files: ["aws_s3", "azure_blob_storage", "csv", "file", "file_tail", "fsevent", "gcp_cloud_storage", "hdfs", "parquet", "sftp"],
  databases: ["azure_cosmosdb", "azure_table_storage", "cassandra", "cockroachdb_changefeed", "cypher", "etcd", "gcp_bigquery_select", "gcp_spanner_cdc", "mongodb", "redis_scan", "sql_raw", "sql_select"],
  http: ["grpc_client", "http_client", "http_server", "socket", "socket_server", "websocket"],
  streams: ["aws_kinesis", "gcp_pubsub", "s2", "kubernetes_watch"],
  queues: ["aws_sqs", "azure_queue_storage", "azure_service_bus_queue"],
  utility: ["batched", "broker", "discord", "dynamic", "generate", "inproc", "read_until", "resource", "sequence", "stdin", "subprocess", "twitter_search"],
};

function fields(node, prefix = "") {
  const out = [];
  for (const child of node.children ?? []) {
    const path = prefix ? `${prefix}.${child.name}` : child.name;
    out.push({
      path,
      type: child.type,
      kind: child.kind,
      secret: Boolean(child.is_secret) || alsoSecret(path),
      advanced: Boolean(child.is_advanced),
      optional: Boolean(child.is_optional),
      default: child.default ?? null,
      description: (child.description ?? "").split("\n")[0].slice(0, 200),
    });
    if (child.type === "object" && child.kind === "scalar") {
      out.push(...fields(child, path));
    }
  }
  return out;
}

const [outPath, rustPath] = process.argv.slice(2);
if (!outPath) {
  console.error("usage: bento list --format json-full | node scripts/bento-inputs.mjs <out.json> [<bento_inputs.rs>]");
  process.exit(1);
}
const listing = JSON.parse(readFileSync(0, "utf8"));
const groupOf = new Map(Object.entries(GROUPS).flatMap(([group, names]) => names.map((name) => [name, group])));
const catalog = listing.inputs
  .filter((input) => !input.status || input.status !== "deprecated")
  .map((input) => ({
    name: input.name,
    group: groupOf.get(input.name) ?? "utility",
    summary: (input.summary ?? "").split("\n")[0].slice(0, 200),
    fields: fields(input.config),
  }))
  .sort((a, b) => a.name.localeCompare(b.name));
const missing = catalog.filter((input) => !groupOf.has(input.name)).map((input) => input.name);
if (missing.length > 0) {
  console.error(`inputs without a group, filed under utility: ${missing.join(", ")}`);
}
writeFileSync(outPath, `${JSON.stringify({ version: listing.version, inputs: catalog }, null, 1)}\n`);
console.log(`${catalog.length} inputs, ${catalog.reduce((n, input) => n + input.fields.length, 0)} fields → ${outPath}`);

// Inputs that end on their own once they have read what there is: a file, a query, a listing.
// A scheduled pipeline may run one; a broker or socket input never ends and stays resident.
const TERMINATING = ["aws_s3", "azure_blob_storage", "azure_table_storage", "cassandra", "csv", "file", "gcp_bigquery_select", "gcp_cloud_storage", "generate", "hdfs", "parquet", "read_until", "redis_scan", "sequence", "sftp", "sql_raw", "sql_select", "stdin", "subprocess"];
const FILE_READERS = ["csv", "file", "file_tail", "parquet"];

// Every secret field with its full path; `[]` stands for each element of an array of objects.
function secretPaths(node, prefix = "") {
  const out = [];
  for (const child of node.children ?? []) {
    const path = prefix ? `${prefix}.${child.name}` : child.name;
    if (child.is_secret) {
      out.push(path);
    }
    if (child.type === "object") {
      out.push(...secretPaths(child, child.kind === "array" ? `${path}[]` : path));
    }
  }
  return out;
}

if (rustPath) {
  const names = catalog.map((input) => input.name);
  const lines = [
    "//! The inputs the pinned runner ships (PL-50): names, the fields Bento marks secret, which",
    "//! inputs end on their own. Generated from `bento list --format json-full` of",
    `//! ghcr.io/warpstreamlabs/bento:${listing.version} by joinedcontext-portal/ui/scripts/bento-inputs.mjs;`,
    "//! change the pin and rerun the script, never edit by hand. The script appends its own",
    "//! `ALSO_SECRET` list to what the runner marks: fields the runner's documentation does not flag",
    "//! and that are credentials all the same, per input, after the tree walk (T-2239).",
    "",
    "/// The runner release this catalog was generated from.",
    `pub const RUNNER_VERSION: &str = ${JSON.stringify(listing.version)};`,
    "",
    "/// Every input name the runner accepts as `spec.type` beside the typed four.",
    `pub const INPUTS: &[&str] = &[${names.map((name) => JSON.stringify(name)).join(", ")}];`,
    "",
    "/// Per input, the field paths the runner documents as secrets; `[]` is every element of an",
    "/// array of objects. A manifest holds a `${VAR}` interpolation there, never a value (MF-35).",
    "pub const SECRET_FIELDS: &[(&str, &[&str])] = &[",
    ...listing.inputs
      .filter((input) => names.includes(input.name))
      .sort((a, b) => a.name.localeCompare(b.name))
      // What the runner marks, then this platform's own additions, deduped: the same order the
      // committed file carries, so a regeneration is a no-op rather than a diff (T-2239).
      .map((input) => {
        const marked = secretPaths(input.config);
        const trimmed = catalog.find((entry) => entry.name === input.name)?.fields ?? [];
        const added = trimmed.filter((field) => field.secret && !marked.includes(field.path)).map((field) => field.path);
        const paths = [...new Set([...marked, ...added])];
        return `    (${JSON.stringify(input.name)}, &[${paths.map((path) => JSON.stringify(path)).join(", ")}]),`;
      }),
    "];",
    "",
    "/// Inputs that end on their own once they have read what there is (a scheduled pipeline may",
    "/// run one, PL-04); every other input is a broker or socket and stays resident.",
    `pub const TERMINATING: &[&str] = &[${TERMINATING.map((name) => JSON.stringify(name)).join(", ")}];`,
    "",
    "/// Inputs that read the runner's files volume; their paths stay under `/data/`.",
    `pub const FILE_READERS: &[&str] = &[${FILE_READERS.map((name) => JSON.stringify(name)).join(", ")}];`,
    "",
  ];
  writeFileSync(rustPath, lines.join("\n"));
  console.log(`rust table → ${rustPath}`);
}
