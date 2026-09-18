import { createContext, useCallback, useContext, useEffect, useMemo, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import type { Middleware } from "openapi-fetch";

// Module-level state: the active workspace name, set by the provider from the URL.
let _active: string | null = null;

export function activeWorkspace(): string | null {
  return _active;
}

export function setActiveWorkspace(name: string | null): void {
  _active = name;
}

/**
 * Middleware that appends `?workspace=<name>` to resource-list/read/write requests when a
 * workspace is active. It matches paths under `/api/v1/projects/{project}/` for the resource
 * kinds listed in the task, and never touches ops, drafts, changes, workspaces or other
 * non-resource routes.
 */
const RESOURCE_PATH_RE =
  /^\/api\/v1\/projects\/[^/]+\/(spaces|endpoints|datasources|pipelines|dashboards|apps|syncsources|datamodels|mappings|policies|subscriptions|csrs|shared|serviceaccounts|layers|uischemas|projections|dataoffers|dataagreements|dataspaceparticipants|ckaninstances|blueprints|agentprofiles|bundles|roles|rolebindings|groups|environments)(\/[^/]+)?$/;

export const workspaceMiddleware: Middleware = {
  onRequest({ request }) {
    const name = activeWorkspace();
    if (!name) return request;
    const url = new URL(request.url);
    if (!RESOURCE_PATH_RE.test(url.pathname)) return request;
    url.searchParams.set("workspace", name);
    return new Request(url.toString(), request);
  },
};

export interface WorkspaceContextValue {
  name: string | null;
  project?: string;
  enter: (name: string) => void;
  leave: () => void;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

export function WorkspaceProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const search = useRouterState({ select: (s) => s.location.search as Record<string, string | undefined> });
  const workspaceParam = search?.workspace ?? null;
  const navigate = useNavigate();

  const queryClient = useQueryClient();
  // Set during render, not in an effect: the pages below fetch in their own effects, which run
  // before this component's, and their first request must already carry the workspace.
  setActiveWorkspace(workspaceParam);
  // What was cached is main's (or the other copy's): entering or leaving reads everything again.
  const previous = useRef(workspaceParam);
  useEffect(() => {
    if (previous.current !== workspaceParam) {
      previous.current = workspaceParam;
      void queryClient.invalidateQueries();
    }
  }, [workspaceParam, queryClient]);

  const enter = useCallback(
    (name: string) => {
      // The Portal's routes declare no search schema; the search is the URL's own record.
      void navigate({
        to: ".",
        search: ((prev: Record<string, unknown>) => ({ ...prev, workspace: name })) as never,
      });
    },
    [navigate],
  );

  const leave = useCallback(() => {
    void navigate({
      to: ".",
      search: ((prev: Record<string, unknown>) => {
        const rest = { ...prev };
        delete rest.workspace;
        return rest;
      }) as never,
    });
  }, [navigate]);

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      name: workspaceParam,
      enter,
      leave,
    }),
    [workspaceParam, enter, leave],
  );

  return <WorkspaceContext value={value}>{children}</WorkspaceContext>;
}

const OUTSIDE: WorkspaceContextValue = { name: null, enter: () => {}, leave: () => {} };

/** The active workspace; outside a provider (a page rendered on its own) there is none. */
export function useWorkspace(): WorkspaceContextValue {
  return useContext(WorkspaceContext) ?? OUTSIDE;
}
