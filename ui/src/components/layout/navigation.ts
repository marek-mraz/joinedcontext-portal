import type { IconName } from "../ui";

/** The plural segments of `/api/v1/projects/{project}/{plural}`, in sidebar order. */
export const NAV_SECTIONS = [
  // The gallery is the primary model of the application, so it is the first thing in the
  // sidebar; everything below it is the expert view of what a flow produced (CC-30).
  { plural: "flows", labelKey: "nav.flows", icon: "flows" },
  { plural: "spaces", labelKey: "nav.spaces", icon: "spaces" },
  // What this project references from other projects' endpoints lives in the Endpoints page's
  // "Shared with this project" section, not in a page of its own (T-0706, EP-15).
  { plural: "endpoints", labelKey: "nav.endpoints", icon: "endpoints" },
  // A source is what a pipeline reads, so it sits in front of the pipelines (MF-35).
  { plural: "datasources", labelKey: "nav.datasources", icon: "datasources" },
  { plural: "pipelines", labelKey: "nav.pipelines", icon: "pipelines" },
  { plural: "dashboards", labelKey: "nav.dashboards", icon: "dashboards" },
  { plural: "apps", labelKey: "nav.apps", icon: "apps" },
  // Where this project's resources are copied or mirrored from; the page that adds one is the
  // only way into it, so it needs an entry of its own (MF-27, T-0790).
  { plural: "syncsources", labelKey: "nav.sync", icon: "refresh" },
  { plural: "assistant", labelKey: "nav.assistant", icon: "chat" },
  // Copies of the project a person changes on the side and brings back as one Change (UI-61).
  { plural: "workspaces", labelKey: "nav.workspaces", icon: "git" },
  { plural: "approvals", labelKey: "nav.approvals", icon: "approvals" },
  // What the reconciler, the pipelines, the gateway, the broker and the catalogue did, in one
  // place, so "is it working" has an answer that is not a Grafana login (UI-31).
  { plural: "activity", labelKey: "nav.activity", icon: "refresh" },
  { plural: "access", labelKey: "nav.access", icon: "access" },
] as const satisfies ReadonlyArray<{ plural: string; labelKey: string; icon: IconName }>;
