import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  Navigate,
  Outlet,
  redirect,
} from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useProjects } from "./api/projects";
import { BrandMark, Shell } from "./components/layout/Shell";
import { EmptyState } from "./components/ui";
import { AllEndpointsPage } from "./routes/AllEndpointsPage";
import { LoginPage } from "./routes/LoginPage";
import { ResourceListPage } from "./routes/ResourceListPage";
import { ApprovalsPage } from "./routes/ApprovalsPage";
import { ApprovalDetailPage } from "./routes/ApprovalDetailPage";
import { ModelsPage } from "./pages/models/ModelsPage";
import { ExplorePage } from "./pages/explore/ExplorePage";
import { CkanPage } from "./pages/ckan/CkanPage";
import { SpaceInside } from "./pages/spaces/SpaceInside";
import { AppPage } from "./pages/apps/AppPage";
import { AssistantPage } from "./pages/assistant/AssistantPage";
import { HandOff } from "./assistant/HandOff";
import type { AuthState } from "./auth/AuthProvider";

export interface RouterContext {
  auth: AuthState;
}

/**
 * What a route without a `$project` shows while the project list is on its way, and when the
 * repository holds none: there is no project to hang a shell on, so the page says so (PF-05).
 */
function NoProject({ pending }: { pending: boolean }): React.JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex min-h-screen flex-col bg-bg font-sans text-fg">
      <header className="flex h-14 items-center border-b border-border bg-surface px-4">
        <BrandMark short />
      </header>
      <main id="main" className="flex flex-1 items-center justify-center p-6">
        {pending ? (
          <p role="status" className="text-body text-fg-muted">
            {t("projects.loading")}
          </p>
        ) : (
          <EmptyState
            icon="spaces"
            title={t("projects.empty.title")}
            description={t("projects.empty.description")}
          />
        )}
      </main>
    </div>
  );
}

/** The shell around a page that belongs to no single project: it opens on the first one. */
function AnyProjectShell({ children }: { children: React.ReactNode }): React.JSX.Element {
  const projects = useProjects();
  const first = projects.data?.[0];
  if (!first) {
    return <NoProject pending={projects.isPending} />;
  }
  return <Shell project={first}>{children}</Shell>;
}

/** `/` goes to the first visible project's spaces; with no project there is nowhere to go. */
function IndexRedirect(): React.JSX.Element {
  const projects = useProjects();
  const first = projects.data?.[0];
  if (!first) {
    return <NoProject pending={projects.isPending} />;
  }
  return <Navigate to="/projects/$project/$plural" params={{ project: first, plural: "spaces" }} />;
}

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
  component: IndexRedirect,
});

const approvalsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/approvals",
  component: function ApprovalsRoute() {
    const { project } = approvalsRoute.useParams();
    return (
      <Shell project={project}>
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
      <Shell project={project}>
        <ApprovalDetailPage project={project} id={id} />
      </Shell>
    );
  },
});

/** The federation playground is gone (UI-28): an old link lands on the project home. */
const playgroundRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/playground",
  beforeLoad: () => {
    throw redirect({ to: "/" });
  },
});

/** Every endpoint of every project in one table (EP-08, EP-44). */
const allEndpointsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/endpoints",
  component: function AllEndpointsRoute() {
    return (
      <AnyProjectShell>
        <AllEndpointsPage />
      </AnyProjectShell>
    );
  },
});

const modelsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/models",
  component: function ModelsRoute() {
    const { project } = modelsRoute.useParams();
    return (
      <Shell project={project}>
        <HandOff>
          <ModelsPage project={project} />
        </HandOff>
      </Shell>
    );
  },
});

const exploreRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/explore",
  // The assistant opens the explorer on what it found (UI-46): a space and an endpoint by name.
  validateSearch: (search: Record<string, unknown>): { space?: string; endpoint?: string } => ({
    space: typeof search.space === "string" ? search.space : undefined,
    endpoint: typeof search.endpoint === "string" ? search.endpoint : undefined,
  }),
  component: function ExploreRoute() {
    const { project } = exploreRoute.useParams();
    const { space, endpoint } = exploreRoute.useSearch();
    return (
      <Shell project={project}>
        <ExplorePage project={project} initialSpace={space} initialEndpoint={endpoint} />
      </Shell>
    );
  },
});

const ckanRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/ckan",
  component: function CkanRoute() {
    const { project } = ckanRoute.useParams();
    return (
      <Shell project={project}>
        <CkanPage project={project} />
      </Shell>
    );
  },
});

/** The Federation page is gone (UI-28): an old link lands on the project's context spaces. */
const federationRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/federation",
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/projects/$project/$plural",
      params: { project: params.project, plural: "spaces" },
    });
  },
});

import { SpaceComplete } from "./pages/spaces/SpaceComplete";

/** Space Complete route (registered before spaceInsideRoute so /spaces/complete is matched statically). */
const spaceCompleteRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/spaces/complete",
  component: function SpaceCompleteRoute() {
    const { project } = spaceCompleteRoute.useParams();
    return (
      <Shell project={project}>
        <HandOff>
          <SpaceComplete project={project} />
        </HandOff>
      </Shell>
    );
  },
});

/** What one Context Space holds: entity types with counts, endpoints, policies (UI-01). */
const spaceInsideRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/spaces/$name",
  component: function SpaceInsideRoute() {
    const { project, name } = spaceInsideRoute.useParams();
    return (
      <Shell project={project}>
        <SpaceInside project={project} name={name} />
      </Shell>
    );
  },
});

/** An application and its runs (AP-68, AP-69, T-0559). */
const appRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/apps/$name",
  component: function AppRoute() {
    const { project, name } = appRoute.useParams();
    return (
      <Shell project={project}>
        <AppPage project={project} name={name} />
      </Shell>
    );
  },
});

/** The assistant workbench (UI-54). */
const assistantRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/assistant",
  component: function AssistantRoute() {
    const { project } = assistantRoute.useParams();
    return (
      <Shell project={project}>
        <AssistantPage project={project} />
      </Shell>
    );
  },
});

// The shared references had a page of their own; they live in the Endpoints page's
// "Shared with this project" section now. The old URL still lands there, so a bookmark and the
// assistant's `navigate` are not broken (T-0706, EP-15).
const sharedRedirectRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/shared",
  component: function SharedRedirectRoute() {
    const { project } = sharedRedirectRoute.useParams();
    return (
      <Navigate
        replace
        to="/projects/$project/$plural"
        params={{ project, plural: "endpoints" }}
        hash="shared-with-project"
      />
    );
  },
});

const resourceListRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/$plural",
  component: function ResourceListRoute() {
    const { project, plural } = resourceListRoute.useParams();
    return (
      <Shell project={project}>
        <HandOff>
          <ResourceListPage project={project} plural={plural} />
        </HandOff>
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
    allEndpointsRoute,
    modelsRoute,
    exploreRoute,
    ckanRoute,
    federationRoute,
    spaceCompleteRoute,
    spaceInsideRoute,
    appRoute,
    assistantRoute,
    sharedRedirectRoute,
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
