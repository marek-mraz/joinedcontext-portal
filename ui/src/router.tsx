import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  Outlet,
  redirect,
} from "@tanstack/react-router";
import { Shell } from "./components/layout/Shell";
import { LoginPage } from "./routes/LoginPage";
import { ResourceListPage } from "./routes/ResourceListPage";
import { ApprovalsPage } from "./routes/ApprovalsPage";
import { ApprovalDetailPage } from "./routes/ApprovalDetailPage";
import { FederationPlayground } from "./components/visualization/FederationPlayground";
import type { AuthState } from "./auth/AuthProvider";

export interface RouterContext {
  auth: AuthState;
}

/** Until a Project list endpoint exists the shell offers the demo project only (T-0205 follows). */
const KNOWN_PROJECTS = ["banskabystrica"];

const rootRoute = createRootRouteWithContext<RouterContext>()({
  component: Outlet,
});

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: LoginPage,
});

/** Everything below this route needs a live session (CC-42, defence in depth). */
const protectedRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "protected",
  beforeLoad: ({ context, location }) => {
    if (context.auth.status === "anonymous") {
      throw redirect({ to: "/login", search: { redirect_to: location.href } });
    }
  },
});

const indexRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({
      to: "/projects/$project/$plural",
      params: { project: KNOWN_PROJECTS[0], plural: "spaces" },
    });
  },
});

const approvalsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/approvals",
  component: function ApprovalsRoute() {
    const { project } = approvalsRoute.useParams();
    return (
      <Shell project={project} projects={KNOWN_PROJECTS}>
        <ApprovalsPage project={project} />
      </Shell>
    );
  },
});

const approvalDetailRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/approvals/$id",
  component: function ApprovalDetailRoute() {
    const { project, id } = approvalDetailRoute.useParams();
    return (
      <Shell project={project} projects={KNOWN_PROJECTS}>
        <ApprovalDetailPage project={project} id={id} />
      </Shell>
    );
  },
});

/** The playground draws its own data; it needs no project and asks the API for nothing. */
const playgroundRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/playground",
  component: function PlaygroundRoute() {
    return (
      <Shell project={KNOWN_PROJECTS[0]} projects={KNOWN_PROJECTS}>
        <FederationPlayground />
      </Shell>
    );
  },
});

const resourceListRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/$plural",
  component: function ResourceListRoute() {
    const { project, plural } = resourceListRoute.useParams();
    return (
      <Shell project={project} projects={KNOWN_PROJECTS}>
        <ResourceListPage project={project} plural={plural} />
      </Shell>
    );
  },
});

export const routeTree = rootRoute.addChildren([
  loginRoute,
  protectedRoute.addChildren([
    indexRoute,
    approvalsRoute,
    approvalDetailRoute,
    playgroundRoute,
    resourceListRoute,
  ]),
]);

export function createPortalRouter() {
  return createRouter({
    routeTree,
    context: { auth: undefined as unknown as AuthState },
    defaultPreload: false,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createPortalRouter>;
  }
}
