import { NAV_SECTIONS } from "../components/layout/navigation";
import { HAND_OFF } from "./HandOff";

type Translate = (key: string, values?: Record<string, string>) => string;

/** Project pages outside the sidebar's sections, by their path segment. */
const PROJECT_PAGES: Record<string, string> = {
  models: "nav.models",
  explore: "nav.explore",
  ckan: "nav.ckan",
};

/**
 * The page a Portal route opens as a person reads it, its title and the resource the route names,
 * never the address itself (UI-45). A route this does not know is "another page".
 */
export function pageOf(route: string, t: Translate): string {
  const [path, query = ""] = route.split("?");
  const segments = path.split("/").filter((segment) => segment !== "");
  const search = new URLSearchParams(query);
  let named = HAND_OFF.map((key) => search.get(key)).find((value) => value !== null && value !== "");
  let title: string | undefined;
  if (path === "/endpoints") {
    title = t("nav.allEndpoints");
  } else if (segments[0] === "projects" && segments.length >= 3) {
    const [, , section, inside] = segments;
    if (section === "spaces" && inside === "complete") {
      title = t("spaces.complete.title");
    } else if (section in PROJECT_PAGES) {
      title = t(PROJECT_PAGES[section]);
    } else {
      const known = NAV_SECTIONS.find((candidate) => candidate.plural === section);
      title = known === undefined ? undefined : t(known.labelKey);
      if (known !== undefined && inside !== undefined) {
        named ??= decodeURIComponent(inside);
      }
    }
  }
  if (title === undefined) {
    return t("assistant.anotherPage");
  }
  return named === undefined || named === null ? title : t("assistant.pageNamed", { page: title, name: named });
}
