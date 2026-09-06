import { Link, useMatchRoute } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { clsx } from "clsx";
import { LanguageSwitcher } from "../LanguageSwitcher";
import { ExportButton } from "../export/ExportButton";
import { useAuth } from "../../auth/AuthProvider";

/** The plural segments of `/api/v1/projects/{project}/{plural}`, in sidebar order. */
export const NAV_SECTIONS = [
  // The gallery is the primary model of the application, so it is the first thing in the
  // sidebar; everything below it is the expert view of what a flow produced (CC-30).
  { plural: "flows", labelKey: "nav.flows" },
  { plural: "spaces", labelKey: "nav.spaces" },
  { plural: "endpoints", labelKey: "nav.endpoints" },
  // A source is what a pipeline reads, so it sits in front of the pipelines (MF-35).
  { plural: "datasources", labelKey: "nav.datasources" },
  { plural: "pipelines", labelKey: "nav.pipelines" },
  { plural: "dashboards", labelKey: "nav.dashboards" },
  { plural: "apps", labelKey: "nav.apps" },
  { plural: "approvals", labelKey: "nav.approvals" },
  { plural: "access", labelKey: "nav.access" },
] as const;

function ProjectSelector({ projects, active }: { projects: string[]; active: string }) {
  const { t } = useTranslation();
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={t("nav.projects")}
          className="inline-flex w-full items-center justify-between rounded border border-border bg-surface px-3 py-1.5 text-sm font-medium text-surface-fg hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {active}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          className="z-50 min-w-[12rem] rounded border border-border bg-surface p-1 text-surface-fg shadow-md"
        >
          {projects.map((project) => (
            <DropdownMenu.Item key={project} asChild>
              <Link
                to="/projects/$project/$plural"
                params={{ project, plural: "spaces" }}
                aria-current={project === active ? "true" : undefined}
                className="flex cursor-pointer items-center rounded px-2 py-1.5 text-sm outline-none hover:bg-surface-subtle focus:bg-surface-subtle"
              >
                {project}
              </Link>
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function UserMenu() {
  const { t } = useTranslation();
  const { identity, signOut } = useAuth();
  if (!identity) {
    return null;
  }
  const display = identity.name ?? identity.username;
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={t("auth.signedInAs", { name: display })}
          className="inline-flex items-center rounded border border-border bg-surface px-3 py-1.5 text-sm font-medium text-surface-fg hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus"
        >
          {display}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="z-50 min-w-[10rem] rounded border border-border bg-surface p-1 text-surface-fg shadow-md"
        >
          <DropdownMenu.Item
            onSelect={() => {
              void signOut();
            }}
            className="flex cursor-pointer items-center rounded px-2 py-1.5 text-sm outline-none hover:bg-surface-subtle focus:bg-surface-subtle"
          >
            {t("auth.signOut")}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export function Shell({
  project,
  projects,
  children,
}: {
  project: string;
  projects: string[];
  children: ReactNode;
}): React.JSX.Element {
  const { t } = useTranslation();
  const matchRoute = useMatchRoute();

  // Approvals has its own routes, so the generic `$plural` match never fires for it.
  const approvalDetail = matchRoute({ to: "/projects/$project/approvals/$id" });
  const onApprovals = Boolean(approvalDetail || matchRoute({ to: "/projects/$project/approvals" }));

  const playgroundActive = Boolean(matchRoute({ to: "/playground" }));
  const modelsActive = Boolean(matchRoute({ to: "/projects/$project/models", params: { project } }));

  const activeSection = NAV_SECTIONS.find((section) =>
    section.plural === "approvals"
      ? onApprovals
      : Boolean(
          matchRoute({
            to: "/projects/$project/$plural",
            params: { project, plural: section.plural },
          }),
        ),
  );

  return (
    <div className="min-h-screen bg-surface font-sans text-surface-fg">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:rounded focus:bg-primary focus:px-3 focus:py-2 focus:text-primary-fg"
      >
        {t("nav.skipToContent")}
      </a>
      <header className="flex items-center justify-between border-b border-border px-4 py-3">
        <h1 className="text-lg font-bold">{t("app.title")}</h1>
        <div className="flex items-center gap-2">
          {/* One click from anywhere in the project, which is the whole of CC-49. */}
          <ExportButton project={project} target={{}} label={t("export.project")} />
          <LanguageSwitcher />
          <UserMenu />
        </div>
      </header>
      <div className="flex">
        <nav
          aria-label={t("nav.main")}
          className="w-56 shrink-0 border-r border-border p-3"
        >
          <ProjectSelector projects={projects} active={project} />
          <ul className="mt-3 space-y-1">
            {NAV_SECTIONS.map((section) => {
              const isActive = section === activeSection;
              const linkProps = {
                "aria-current": isActive ? ("page" as const) : undefined,
                className: clsx(
                  "block rounded px-2 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus",
                  isActive && "bg-surface-subtle font-semibold",
                ),
              };
              return (
                <li key={section.plural}>
                  {/* Approvals has a route of its own; linking it through the generic
                      template would resolve to that route anyway, with a router warning. */}
                  {section.plural === "approvals" ? (
                    <Link to="/projects/$project/approvals" params={{ project }} {...linkProps}>
                      {t(section.labelKey)}
                    </Link>
                  ) : (
                    <Link
                      to="/projects/$project/$plural"
                      params={{ project, plural: section.plural }}
                      {...linkProps}
                    >
                      {t(section.labelKey)}
                    </Link>
                  )}
                </li>
              );
            })}
          </ul>
          {/* Neither is a section of the resource API: the model editor writes LinkML into the
              repository rather than a manifest of its own, and the playground is a drawing of
              federation that runs entirely in the browser. Both hang below the list. */}
          <ul className="mt-3 space-y-1 border-t border-border pt-3">
            <li>
              <Link
                to="/projects/$project/models"
                params={{ project }}
                aria-current={modelsActive ? "page" : undefined}
                className={clsx(
                  "block rounded px-2 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus",
                  modelsActive && "bg-surface-subtle font-semibold",
                )}
              >
                {t("nav.models")}
              </Link>
            </li>
            <li>
              <Link
                to="/playground"
                aria-current={playgroundActive ? "page" : undefined}
                className={clsx(
                  "block rounded px-2 py-1.5 text-sm hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus",
                  playgroundActive && "bg-surface-subtle font-semibold",
                )}
              >
                {t("nav.playground")}
              </Link>
            </li>
          </ul>
        </nav>
        <main id="main" className="min-w-0 flex-1 p-6">
          <nav aria-label={t("nav.breadcrumb")} className="mb-4 text-sm">
            <ol className="flex flex-wrap items-center gap-1">
              <li>
                <Link
                  to="/projects/$project/$plural"
                  params={{ project, plural: "spaces" }}
                  className="rounded underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
                >
                  {project}
                </Link>
              </li>
              {activeSection ? (
                <li className="flex items-center gap-1">
                  <span aria-hidden="true">/</span>
                  {approvalDetail ? (
                    <>
                      <Link
                        to="/projects/$project/approvals"
                        params={{ project }}
                        className="rounded underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
                      >
                        {t(activeSection.labelKey)}
                      </Link>
                      <span aria-hidden="true">/</span>
                      <span aria-current="page" className="font-mono font-semibold">
                        {approvalDetail.id}
                      </span>
                    </>
                  ) : (
                    <span aria-current="page" className="font-semibold">
                      {t(activeSection.labelKey)}
                    </span>
                  )}
                </li>
              ) : null}
            </ol>
          </nav>
          {children}
        </main>
      </div>
    </div>
  );
}
