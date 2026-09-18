import { parse as parseYaml } from "yaml";

/**
 * The name a typed manifest carries, or "" while it has none or does not parse. The access
 * dialogs start from an example without a name and keep Propose disabled until there is one, so
 * an untouched example never becomes a Change (T-1492, PF-57).
 */
export function manifestName(source: string): string {
  try {
    const name = (parseYaml(source) as { metadata?: { name?: unknown } } | null)?.metadata?.name;
    return typeof name === "string" ? name.trim() : "";
  } catch {
    return "";
  }
}
