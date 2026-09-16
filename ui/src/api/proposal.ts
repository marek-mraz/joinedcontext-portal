import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "./client";
import { isChange } from "./manifest";
import type { Change } from "./manifest";

/** One PUT or POST of a manifest, as the endpoints page does it; the result is a Change. */
export function useProposal(project: string, plural: string, onChange: (change: Change) => void) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: async ({
      body,
      create,
      bundle = [],
    }: {
      body: { metadata: { name: string } };
      create: boolean;
      /** Drafted manifests the resource needs, proposed with it as one Change. */
      bundle?: unknown[];
    }) => {
      setError(null);
      if (bundle.length > 0) {
        return unwrap<unknown>(
          await api.POST("/api/v1/projects/{project}/import", {
            params: { path: { project } },
            body: { manifests: [...bundle, body] } as never,
          }),
        );
      }
      const result = create
        ? await api.POST("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural } },
            body: body as never,
          })
        : await api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
            params: { path: { project, plural, name: body.metadata.name } },
            body: body as never,
          });
      return unwrap(result);
    },
    onSuccess: (result, { bundle = [] }) => {
      if (isChange(result)) {
        onChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, plural) });
      if (bundle.length > 0) {
        void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "layers") });
      }
    },
    onError: (err) => {
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });
  return { mutation, error, reset: () => setError(null) };
}
