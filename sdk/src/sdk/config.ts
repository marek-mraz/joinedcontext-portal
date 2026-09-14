export interface JcUser {
  id: string;
  name?: string;
  email?: string;
  roles?: string[];
}

export interface JcConfig {
  slug: string;
  orgDomain: string;
  space: string;
  transport: "origin" | "bridge";
  appName?: string;
  endpointName?: string;
  basemap?: string;
  language?: string;
  user?: JcUser | null;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConfigError";
  }
}

const SLUG_RE = /^[a-z0-9]{1,64}$/;
const APP_NAME_RE = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

export function parseConfig(input: unknown): JcConfig {
  const errors: string[] = [];

  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    throw new ConfigError("Config must be an object");
  }

  const raw = input as Record<string, unknown>;

  if (typeof raw.slug !== "string" || !SLUG_RE.test(raw.slug)) {
    errors.push("slug: must match ^[a-z0-9]{1,64}$");
  }

  if (typeof raw.orgDomain !== "string" || raw.orgDomain.trim() === "" || /[:/]/.test(raw.orgDomain)) {
    errors.push("orgDomain: must be non-empty and contain no ':' or '/'");
  }

  if (typeof raw.space !== "string" || raw.space.trim() === "" || /[:/]/.test(raw.space)) {
    errors.push("space: must be non-empty and contain no ':' or '/'");
  }

  if (raw.transport !== "origin" && raw.transport !== "bridge") {
    errors.push("transport: must be 'origin' or 'bridge'");
  }

  if (raw.transport === "origin") {
    if (typeof raw.appName !== "string" || !APP_NAME_RE.test(raw.appName)) {
      errors.push("appName: required when transport is 'origin', must match ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$");
    }
  } else if (raw.appName !== undefined) {
    if (typeof raw.appName !== "string" || !APP_NAME_RE.test(raw.appName)) {
      errors.push("appName: must match ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$");
    }
  }

  if (raw.basemap !== undefined) {
    if (typeof raw.basemap !== "string") {
      errors.push("basemap: must be an http(s) URL");
    } else {
      try {
        const u = new URL(raw.basemap);
        if (u.protocol !== "https:" && u.protocol !== "http:") {
          errors.push("basemap: must be an http(s) URL");
        }
      } catch {
        errors.push("basemap: must be an http(s) URL");
      }
    }
  }

  if (raw.endpointName !== undefined && typeof raw.endpointName !== "string") {
    errors.push("endpointName: must be a string");
  }

  if (raw.language !== undefined && typeof raw.language !== "string") {
    errors.push("language: must be a string");
  }

  let user: JcUser | null | undefined = undefined;
  if (raw.user !== undefined && raw.user !== null) {
    if (typeof raw.user !== "object" || Array.isArray(raw.user)) {
      errors.push("user: must be an object or null");
    } else {
      const u = raw.user as Record<string, unknown>;
      if (typeof u.id !== "string" || u.id === "") {
        errors.push("user.id: must be a non-empty string");
      }
      if (u.name !== undefined && typeof u.name !== "string") {
        errors.push("user.name: must be a string");
      }
      if (u.email !== undefined && typeof u.email !== "string") {
        errors.push("user.email: must be a string");
      }
      if (u.roles !== undefined && (!Array.isArray(u.roles) || !u.roles.every((r) => typeof r === "string"))) {
        errors.push("user.roles: must be an array of strings");
      }
      user = raw.user as JcUser;
    }
  } else if (raw.user === null) {
    user = null;
  }

  if (errors.length > 0) {
    throw new ConfigError(errors.join(", "));
  }

  const slug = raw.slug as string;
  return {
    slug,
    orgDomain: raw.orgDomain as string,
    space: raw.space as string,
    transport: raw.transport as "origin" | "bridge",
    appName: raw.appName as string | undefined,
    endpointName: (typeof raw.endpointName === "string" && raw.endpointName.trim() !== "" ? raw.endpointName : slug),
    basemap: raw.basemap as string | undefined,
    language: typeof raw.language === "string" && raw.language !== "" ? raw.language : "en",
    user,
  };
}

export function readConfig(doc?: Document): JcConfig {
  const d = doc ?? (typeof document !== "undefined" ? document : undefined);
  if (!d) {
    throw new ConfigError("Document is not available");
  }
  const element = d.getElementById("jc-config");
  if (!element || !element.textContent?.trim()) {
    throw new ConfigError("Missing or empty #jc-config element");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(element.textContent);
  } catch (err) {
    throw new ConfigError(`Invalid JSON in #jc-config: ${err instanceof Error ? err.message : String(err)}`);
  }
  return parseConfig(parsed);
}
