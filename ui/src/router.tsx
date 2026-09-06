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
  protectedRoute.addChildren([indexRoute, resourceListRoute]),
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
