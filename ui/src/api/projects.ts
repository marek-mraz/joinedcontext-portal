import { useQuery } from "@tanstack/react-query";
import { api, queryKeys, unwrap } from "./client";

/** The projects the configuration repository holds, as `GET /api/v1/projects` lists them (PF-05). */
export function useProjects() {
  return useQuery({
    queryKey: queryKeys.projects(),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects")),
    select: (list) => list.items.map((item) => item.name),
  });
}
