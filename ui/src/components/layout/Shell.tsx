import { useState } from "react";
import { Link, useMatchRoute } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { LanguageSwitcher } from "../LanguageSwitcher";
import { ExportButton } from "../export/ExportButton";
import { useAuth } from "../../auth/AuthProvider";
import { logoUrl, useBranding } from "../../branding";
import { Button, Icon, Menu, MenuContent, MenuItem, MenuLabel, MenuSeparator, MenuTrigger } from "../ui";
import type { IconName } from "../ui";

/** The plural segments of `/api/v1/projects/{project}/{plural}`, in sidebar order. */
export const NAV_SECTIONS = [
  // The gallery is the primary model of the application, so it is the first thing in the
  // sidebar; everything below it is the expert view of what a flow produced (CC-30).
  { plural: "flows", labelKey: "nav.flows", icon: "flows" },
  { plural: "spaces", labelKey: "nav.spaces", icon: "spaces" },
  { plural: "endpoints", labelKey: "nav.endpoints", icon: "endpoints" },
  // A source is what a pipeline reads, so it sits in front of the pipelines (MF-35).
  { plural: "datasources", labelKey: "nav.datasources", icon: "datasources" },
  { plural: "pipelines", labelKey: "nav.pipelines", icon: "pipelines" },
  { plural: "dashboards", labelKey: "nav.dashboards", icon: "dashboards" },
  { plural: "apps", labelKey: "nav.apps", icon: "apps" },
  { plural: "approvals", labelKey: "nav.approvals", icon: "approvals" },
  { plural: "access", labelKey: "nav.access", icon: "access" },
] as const satisfies ReadonlyArray<{ plural: string; labelKey: string; icon: IconName }>;

const NAV_LINK =
  "focus-ring-inset flex items-center gap-2.5 rounded-md px-2.5 py-2 text-body text-fg-muted transition-colors hover:bg-surface-muted hover:text-fg";
const NAV_LINK_ACTIVE = "bg-primary-soft font-semibold text-primary-soft-fg hover:bg-primary-soft hover:text-primary-soft-fg";

function ProjectSelector({ projects, active }: { projects: string[]; active: string }) {
  const { t } = useTranslation();
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          aria-label={t("nav.projects")}
          className="focus-ring flex w-full items-center gap-2.5 rounded-md border border-border bg-surface px-2.5 py-2 text-left shadow-1 hover:bg-surface-subtle"
        >
          <span className="inline-flex size-7 shrink-0 items-center justify-center rounded-md bg-primary text-caption font-bold uppercase text-primary-fg">
            {active.slice(0, 2)}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-body font-semibold text-fg">{active}</span>
          </span>
          <Icon name="chevronDown" className="size-4 text-fg-subtle" />
        </button>
      </MenuTrigger>
      <MenuContent align="start" className="w-[var(--radix-dropdown-menu-trigger-width)]">
        <MenuLabel>{t("nav.projects")}</MenuLabel>
        {projects.map((project) => (
          <MenuItem key={project} asChild>
            <Link
              to="/projects/$project/$plural"
              params={{ project, plural: "spaces" }}
              aria-current={project === active ? "true" : undefined}
            >
              {project === active ? <Icon name="check" className="size-4" /> : <span className="size-4" />}
              {project}
            </Link>
          </MenuItem>
        ))}
      </MenuContent>
    </Menu>
  );
}

function UserMenu() {
  const { t } = useTranslation();
  const { identity, signOut } = useAuth();
  if (!identity) {
    return null;
  }
  const display = identity.name ?? identity.username;
  const initial = display.trim().charAt(0).toUpperCase();
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          aria-label={t("auth.signedInAs", { name: display })}
          className="focus-ring inline-flex h-9 items-center gap-2 rounded-md px-1.5 hover:bg-surface-muted"
        >
          <span className="inline-flex size-7 items-center justify-center rounded-full bg-primary-soft text-caption font-bold text-primary-soft-fg">
            {initial}
          </span>
          <span className="hidden max-w-[10rem] truncate text-body font-medium text-fg sm:block">
            {display}
          </span>
          <Icon name="chevronDown" className="hidden size-4 text-fg-subtle sm:block" />
        </button>
      </MenuTrigger>
      <MenuContent align="end" className="min-w-[13rem]">
        <MenuLabel>
          <span className="block truncate text-body font-medium text-fg">{display}</span>
          {identity.name ? <span className="block truncate font-mono">{identity.username}</span> : null}
        </MenuLabel>
        <MenuSeparator />
        <MenuItem
          onSelect={() => {
            void signOut();
          }}
        >
          <Icon name="logout" className="size-4" />
          {t("auth.signOut")}
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}

/** The instance name and its logo, which is what every page is titled with (UI-30). */
export function BrandMark({ short = false }: { short?: boolean }): React.JSX.Element {
  const branding = useBranding();
  const logo = logoUrl(branding);
  const name = short ? (branding.shortName ?? branding.instanceName) : branding.instanceName;
  return (
    <span className="inline-flex items-center gap-2.5">
      {logo ? (
        <img src={logo} alt="" aria-hidden="true" className="h-7 w-auto max-w-[8rem] object-contain" />
      ) : (
        <span
          aria-hidden="true"
          className="inline-flex size-7 items-center justify-center rounded-md bg-primary text-caption font-bold text-primary-fg"
        >
          {name.trim().charAt(0).toUpperCase()}
        </span>
      )}
      <span className="font-heading font-semibold tracking-tight">{name}</span>
    </span>
  );
}

function navLinkClass(active: boolean): string {
  return clsx(NAV_LINK, active && NAV_LINK_ACTIVE);
}

function NavLabel({ icon, label }: { icon: IconName; label: string }) {
  return (
    <>
      <Icon name={icon} className="size-[18px]" />
      <span className="truncate">{label}</span>
    </>
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
  const branding = useBranding();
  const matchRoute = useMatchRoute();
  // The sidebar is a drawer on a phone; a navigation closes it.
  const [navOpen, setNavOpen] = useState(false);
  const closeNav = () => setNavOpen(false);

  // Approvals has its own routes, so the generic `$plural` match never fires for it.
  const approvalDetail = matchRoute({ to: "/projects/$project/approvals/$id" });
  const onApprovals = Boolean(approvalDetail || matchRoute({ to: "/projects/$project/approvals" }));

  const playgroundActive = Boolean(matchRoute({ to: "/playground" }));
  const modelsActive = Boolean(matchRoute({ to: "/projects/$project/models", params: { project } }));
  const ckanActive = Boolean(matchRoute({ to: "/projects/$project/ckan", params: { project } }));
  const federationActive = Boolean(
    matchRoute({ to: "/projects/$project/federation", params: { project } }),
  );

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
    <div className="flex min-h-screen flex-col bg-bg font-sans text-fg">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50 focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-primary-fg"
      >
        {t("nav.skipToContent")}
      </a>

      <header className="sticky top-0 z-30 flex h-14 items-center gap-2 border-b border-border bg-surface/95 px-3 backdrop-blur sm:px-4">
        <Button
          variant="ghost"
          size="sm"
          className="md:hidden"
          aria-label={t("nav.menu")}
          aria-expanded={navOpen}
          aria-controls="portal-sidebar"
          onClick={() => setNavOpen((open) => !open)}
          icon={<Icon name={navOpen ? "close" : "menu"} className="size-5" />}
        />
        <Link
          to="/projects/$project/$plural"
          params={{ project, plural: "spaces" }}
          className="focus-ring rounded-md text-body text-fg"
        >
          <BrandMark short />
        </Link>
        <div className="ml-auto flex items-center gap-1">
          {/* One click from anywhere in the project, which is the whole of CC-49. */}
          <ExportButton
            project={project}
            target={{}}
            label={t("export.project")}
            variant="ghost"
            size="sm"
            className="hidden sm:inline-flex"
          />
          <LanguageSwitcher />
          <UserMenu />
        </div>
      </header>

      <div className="flex flex-1">
        {navOpen ? (
          <div
            aria-hidden="true"
            onClick={closeNav}
            className="fixed inset-0 top-14 z-30 bg-overlay md:hidden"
          />
        ) : null}
        <nav
          id="portal-sidebar"
          aria-label={t("nav.main")}
          className={clsx(
            "z-40 w-sidebar shrink-0 flex-col gap-4 overflow-y-auto border-r border-border bg-surface p-3",
            "md:sticky md:top-14 md:flex md:h-[calc(100vh-3.5rem)]",
            navOpen ? "fixed bottom-0 left-0 top-14 flex shadow-3" : "hidden",
          )}
        >
          <ProjectSelector projects={projects} active={project} />
          <ul className="flex flex-col gap-0.5">
            {NAV_SECTIONS.map((section) => {
              const isActive = section === activeSection;
              const body = <NavLabel icon={section.icon} label={t(section.labelKey)} />;
              return (
                <li key={section.plural}>
                  {/* Approvals has a route of its own; linking it through the generic
                      template would resolve to that route anyway, with a router warning. */}
                  {section.plural === "approvals" ? (
                    <Link
                      to="/projects/$project/approvals"
                      params={{ project }}
                      onClick={closeNav}
                      aria-current={isActive ? "page" : undefined}
                      className={navLinkClass(isActive)}
                    >
                      {body}
                    </Link>
                  ) : (
                    <Link
                      to="/projects/$project/$plural"
                      params={{ project, plural: section.plural }}
                      onClick={closeNav}
                      aria-current={isActive ? "page" : undefined}
                      className={navLinkClass(isActive)}
                    >
                      {body}
                    </Link>
                  )}
                </li>
              );
            })}
          </ul>
          {/* Neither is a section of the resource API: the model editor writes LinkML into the
              repository rather than a manifest of its own, and the playground is a drawing of
              federation that runs entirely in the browser. Both hang below the list. */}
          <ul className="flex flex-col gap-0.5 border-t border-border pt-3">
            <li>
              <Link
                to="/projects/$project/models"
                params={{ project }}
                onClick={closeNav}
                aria-current={modelsActive ? "page" : undefined}
                className={navLinkClass(modelsActive)}
              >
                <NavLabel icon="models" label={t("nav.models")} />
              </Link>
            </li>
            <li>
              <Link
                to="/projects/$project/ckan"
                params={{ project }}
                onClick={closeNav}
                aria-current={ckanActive ? "page" : undefined}
                className={navLinkClass(ckanActive)}
              >
                <NavLabel icon="ckan" label={t("nav.ckan")} />
              </Link>
            </li>
            <li>
              <Link
                to="/projects/$project/federation"
                params={{ project }}
                onClick={closeNav}
                aria-current={federationActive ? "page" : undefined}
                className={navLinkClass(federationActive)}
              >
                <NavLabel icon="federation" label={t("nav.federation")} />
              </Link>
            </li>
            <li>
              <Link
                to="/playground"
                onClick={closeNav}
                aria-current={playgroundActive ? "page" : undefined}
                className={navLinkClass(playgroundActive)}
              >
                <NavLabel icon="playground" label={t("nav.playground")} />
              </Link>
            </li>
          </ul>
        </nav>

        <main id="main" className="min-w-0 flex-1">
          <div className="mx-auto flex max-w-content flex-col gap-section px-4 py-5 sm:px-gutter sm:py-6">
            <nav aria-label={t("nav.breadcrumb")} className="text-caption text-fg-muted">
              <ol className="flex flex-wrap items-center gap-1">
                <li>
                  <Link
                    to="/projects/$project/$plural"
                    params={{ project, plural: "spaces" }}
                    className="focus-ring rounded-sm hover:text-fg hover:underline"
                  >
                    {project}
                  </Link>
                </li>
                {activeSection ? (
                  <li className="flex items-center gap-1">
                    <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />
                    {approvalDetail ? (
                      <>
                        <Link
                          to="/projects/$project/approvals"
                          params={{ project }}
                          className="focus-ring rounded-sm hover:text-fg hover:underline"
                        >
                          {t(activeSection.labelKey)}
                        </Link>
                        <Icon name="chevronRight" className="size-3.5 text-fg-subtle" />
                        <span aria-current="page" className="font-mono font-medium text-fg">
                          {approvalDetail.id}
                        </span>
                      </>
                    ) : (
                      <span aria-current="page" className="font-medium text-fg">
                        {t(activeSection.labelKey)}
                      </span>
                    )}
                  </li>
                ) : null}
              </ol>
            </nav>
            {children}
          </div>
        </main>
      </div>

      {branding.organisation || branding.contactEmail ? (
        <footer className="border-t border-border bg-surface px-4 py-3 text-caption text-fg-muted sm:px-gutter">
          {branding.organisation ? <span>{branding.organisation}</span> : null}
          {branding.organisation && branding.contactEmail ? (
            <span aria-hidden="true"> · </span>
          ) : null}
          {branding.contactEmail ? (
            <a
              href={`mailto:${branding.contactEmail}`}
              className="focus-ring rounded-sm underline hover:text-fg hover:no-underline"
            >
              {branding.contactEmail}
            </a>
          ) : null}
        </footer>
      ) : null}
    </div>
  );
}
