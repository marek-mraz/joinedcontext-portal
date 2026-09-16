import { useQuery } from "@tanstack/react-query";
import { api, queryKeys, unwrap } from "./client";
import { asManifests } from "./manifest";
import { useBranding } from "../branding";

/** The projects the configuration repository holds, as `GET /api/v1/projects` lists them (PF-05). */
export function useProjects() {
  return useQuery({
    queryKey: queryKeys.projects(),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects")),
    select: (list) => list.items.map((item) => item.name),
  });
}

/**
 * The organization's domain, in the order the API itself resolves it (`api::assistant::org_domain`):
 * the `Organization` manifest of the repository, then the installation's branding, then the
 * project name. A model minted under a fabricated `<project>.sk` names IRIs nobody owns (T-0794).
 */
export function useOrgDomain(project: string): string {
  const branding = useBranding();
  const organizations = useQuery({
    queryKey: queryKeys.list(project, "organizations"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "organizations" } },
        }),
      ),
  });
  const first = asManifests(organizations.data?.items ?? [])[0];
  const domain = (first?.spec as { domain?: string } | undefined)?.domain;
  return domain || branding.orgDomain || project;
}
