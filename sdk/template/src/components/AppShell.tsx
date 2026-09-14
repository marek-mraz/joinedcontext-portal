import { useEffect, useState, type ReactNode } from "react";
import { useMe } from "@joinedcontext/sdk";
import { Empty, ErrorBoundary } from "./states";

export interface Page {
  id: string;
  label: string;
  render: () => ReactNode;
}

export function navigate(id: string): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("jc-navigate", { detail: id }));
  }
}

function resolveInitialPage(pages: Page[], initial?: string): string {
  if (typeof window !== "undefined" && window.location && window.location.hash) {
    const hashId = window.location.hash.replace(/^#\/?/, "");
    if (pages.some((p) => p.id === hashId)) {
      return hashId;
    }
  }
  if (initial && pages.some((p) => p.id === initial)) {
    return initial;
  }
  return pages[0]?.id ?? "";
}

export function AppShell({
  title,
  pages,
  actions,
  initial,
}: {
  title: string;
  pages: Page[];
  actions?: ReactNode;
  initial?: string;
}): React.JSX.Element {
  const user = useMe();
  const [activeId, setActiveId] = useState(() => resolveInitialPage(pages, initial));

  const selectPage = (id: string) => {
    setActiveId(id);
    try {
      window.history.replaceState(null, "", `#/${id}`);
    } catch {
      // sandboxed preview may refuse
    }
  };

  useEffect(() => {
    const onNavigate = (e: Event) => {
      const custom = e as CustomEvent<string>;
      const targetId = custom.detail;
      if (pages.some((p) => p.id === targetId)) {
        setActiveId(targetId);
        try {
          window.history.replaceState(null, "", `#/${targetId}`);
        } catch {
          // sandboxed preview may refuse
        }
      }
    };

    const onHashChange = () => {
      const hashId = window.location.hash.replace(/^#\/?/, "");
      if (pages.some((p) => p.id === hashId)) {
        setActiveId(hashId);
      }
    };

    window.addEventListener("jc-navigate", onNavigate);
    window.addEventListener("hashchange", onHashChange);
    return () => {
      window.removeEventListener("jc-navigate", onNavigate);
      window.removeEventListener("hashchange", onHashChange);
    };
  }, [pages]);

  const activePage = pages.find((p) => p.id === activeId) ?? pages[0];

  return (
    <div className="jc-shell">
      <header className="jc-header">
        <h1>{title}</h1>
        <nav aria-label="Pages">
          {pages.map((page) => (
            <button
              key={page.id}
              type="button"
              aria-current={page.id === activePage?.id ? "page" : undefined}
              onClick={() => selectPage(page.id)}
            >
              {page.label}
            </button>
          ))}
        </nav>
        {actions}
        <span className="jc-user">{user?.name}</span>
      </header>
      <main className="jc-main">
        {activePage ? (
          <ErrorBoundary key={activePage.id}>{activePage.render()}</ErrorBoundary>
        ) : (
          <Empty>No pages.</Empty>
        )}
      </main>
    </div>
  );
}
