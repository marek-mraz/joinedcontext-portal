import { useQuery } from "@tanstack/react-query";
import { api, queryKeys, unwrap } from "./client";
import type { components } from "./schema";

/** What the caller may do in one project, from the bindings of the organization repository (PF-50). */
export type Effective = components["schemas"]["Effective"];
export type Verb = "propose" | "approve" | "delete";

export interface Rule {
  kinds?: string[];
  verbs?: string[];
}

/** A grant allows a verb on a kind when its rule names both; `*` matches any kind. */
export function allows(effective: Effective | undefined, kind: string, verb: Verb): boolean {
  // Not (yet) a permissions document: the control shows and the API decides (PF-51 says the
  // UI is never the point of enforcement). Only a document that lists no grant hides it.
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return true;
  }
  return effective.grants.some((grant) => {
    const rule = grant.rule as Rule;
    return (kind === "*" || rule.kinds?.includes(kind)) && Boolean(rule.verbs?.includes(verb));
  });
}

/**
 * The caller's effective permissions in `project` (T-0526). A convenience for the UI: a
 * control renders only when the API would honour it, and the API decides on its own.
 */
export function usePermissions(project: string) {
  const query = useQuery({
    queryKey: queryKeys.permissions(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/permissions/me", {
          params: { path: { project } },
        }),
      ),
    enabled: project !== "",
    // A fresh Portal pod answers with no grant until its first mirror sync; a page opened in
    // that window would otherwise keep every control disabled until a reload (T-0630).
    refetchInterval: (query) => {
      const data = query.state.data;
      return data &&
        Array.isArray(data.grants) &&
        data.grants.length === 0 &&
        data.bootstrap !== true
        ? 5000
        : false;
    },
  });
  return {
    data: query.data,
    isLoading: query.isPending,
    can: (kind: string, verb: Verb) => allows(query.data, kind, verb),
  };
}
