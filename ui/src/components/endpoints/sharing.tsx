import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import type { JsonSchema } from "../forms/types";
import { DNS1123, SLUG_PATTERN } from "../../schemas/kinds";
import { Badge } from "../ui";
import type { BadgeTone } from "../ui";

/**
 * Who an Endpoint is shared with, the way the gateway reads it (Architecture/04 §5, EP-14,
 * EP-15): `project-list` admits the owner and the listed projects, `organization` every
 * project of the repository, `public` everyone.
 */
export interface Sharing {
  audience: string;
  allowedProjects: string[];
}

export const SPACE_LABEL = "joinedcontext.com/space";

export const AUDIENCE_TONE: Record<string, BadgeTone> = {
  "project-list": "neutral",
  organization: "info",
  public: "warning",
};

export function sharingOf(endpoint: Manifest): Sharing {
  const spec = endpoint.spec as { audience?: string; allowedProjects?: string[] };
  return {
    audience: spec.audience ?? "project-list",
    allowedProjects: Array.isArray(spec.allowedProjects) ? spec.allowedProjects : [],
  };
}

export function spaceOf(endpoint: Manifest): string | undefined {
  const space = refName((endpoint.spec as { contextSpaceRef?: unknown }).contextSpaceRef);
  return space !== "" ? space : endpoint.metadata.labels?.[SPACE_LABEL];
}

/** Whether `project` may call an endpoint `owner` publishes: the gateway's own rule. */
export function admits(endpoint: Manifest, owner: string, project: string): boolean {
  if (owner === project) {
    return true;
  }
  const { audience, allowedProjects } = sharingOf(endpoint);
  switch (audience) {
    case "organization":
    case "public":
      return true;
    case "project-list":
      return allowedProjects.includes(project);
    default:
      return false;
  }
}

/** The audience as a chip, and for `project-list` the projects it names, so the owner sees who. */
export function SharedWithBadge({ endpoint }: { endpoint: Manifest }): JSX.Element {
  const { t } = useTranslation();
  const { audience, allowedProjects } = sharingOf(endpoint);
  const listed = audience === "project-list";
  return (
    <div className="flex flex-col gap-1">
      <Badge tone={AUDIENCE_TONE[audience] ?? "neutral"}>{t(`endpoints.audience.${audience}`)}</Badge>
      {listed && allowedProjects.length > 0 ? (
        <ul
          aria-label={t("endpoints.sharedWith")}
          className="flex flex-wrap items-center gap-1 text-caption text-fg-muted"
        >
          {allowedProjects.map((name) => (
            <li key={name}>
              <Badge mono>{name}</Badge>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

/** A DNS-1123 label out of whatever parts name the thing: lowercase, dashes, 63 characters. */
export function dns1123(...parts: string[]): string {
  return parts
    .join("-")
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/-+/g, "-")
    .slice(0, 63)
    .replace(/^-+|-+$/g, "");
}

/** `SharedSpaceReference`, field for field with jc-core's `SharedSpaceReferenceSpec` (EP-15). */
export const sharedSpaceReferenceSchema: JsonSchema = {
  type: "object",
  required: ["endpointSlug", "alias"],
  additionalProperties: false,
  properties: {
    endpointSlug: { type: "string", pattern: SLUG_PATTERN },
    alias: { type: "string", pattern: DNS1123, maxLength: 63 },
  },
};

/**
 * The manifest "Use in this project" proposes: one `SharedSpaceReference` in the consumer
 * project, named after the source project and endpoint, its alias after the source project
 * and space so it never shadows a space of the consumer's own (Architecture/04 §5).
 */
export function referenceManifest(project: string, source: string, endpoint: Manifest) {
  const spec = endpoint.spec as { slug?: string };
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "SharedSpaceReference",
    metadata: {
      name: dns1123(source, endpoint.metadata.name),
      namespace: project,
    },
    spec: {
      endpointSlug: spec.slug ?? "",
      alias: dns1123(source, spaceOf(endpoint) ?? endpoint.metadata.name),
    },
  };
}

/** The reference of `project` that points at `slug`, when one is declared already. */
export function referenceTo(references: Manifest[], slug: string): Manifest | undefined {
  return references.find(
    (reference) => (reference.spec as { endpointSlug?: string }).endpointSlug === slug,
  );
}
