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
  { plural: "approvals", labelKey: "nav.approvals", icon: "approvals" },
  { plural: "access", labelKey: "nav.access", icon: "access" },
] as const satisfies ReadonlyArray<{ plural: string; labelKey: string; icon: IconName }>;
