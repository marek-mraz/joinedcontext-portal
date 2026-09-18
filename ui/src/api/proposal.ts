import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "./client";
import { isChange } from "./manifest";
import type { Change } from "./manifest";

/** What a dry run answers, as far as the check before a proposal reads it. */
type CheckAnswer = {
  valid?: boolean;
  verdict?: { ok?: boolean; findings?: { message?: string }[] };
};

/**
 * Checks a manifest, then proposes it (PF-57, T-0956). Every proposal needs a fresh green check,
 * whatever door it comes through, so a page that has no Check button of its own runs the dry run
 * on the same route and verb, with the same body, and proposes only when it is green; a red check
 * stops here with what it found, and nothing is proposed.
 */
export async function proposeChecked(
  project: string,
  plural: string,
  body: { metadata: { name: string } },
  create: boolean,
): Promise<unknown> {
  const send = (dryRun: boolean) => {
    const query = dryRun ? { dryRun: "All" } : undefined;
    return create
      ? api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural }, query },
          body: body as never,
        })
      : api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural, name: body.metadata.name }, query },
          body: body as never,
        });
  };
  const check = (await unwrap(await send(true))) as CheckAnswer;
  if (check.valid === false || check.verdict?.ok === false) {
    const found = (check.verdict?.findings ?? [])
      .map((finding) => finding.message)
      .filter((message): message is string => Boolean(message));
    throw new ApiError(422, found.join("; ") || "the check found problems", {
      type: "about:blank",
      title: "The check found problems",
      status: 422,
      detail: found.join("; ") || "the check found problems",
    });
  }
  return unwrap(await send(false));
}

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
        // The dry run is the bundle's check, and the import needs it first (PF-57, T-1460).
        const manifests = { manifests: [...bundle, body] } as never;
        unwrap<unknown>(
          await api.POST("/api/v1/projects/{project}/import", {
            params: { path: { project }, query: { dryRun: "All" } },
            body: manifests,
          }),
        );
        return unwrap<unknown>(
          await api.POST("/api/v1/projects/{project}/import", {
            params: { path: { project } },
            body: manifests,
          }),
        );
      }
      return proposeChecked(project, plural, body, create);
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
