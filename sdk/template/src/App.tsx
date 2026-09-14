import { useMemo } from "react";
import { AppShell, Empty, Loading, Problem, useClient, useSchema } from "@joinedcontext/sdk";
import type { Page } from "@joinedcontext/sdk";
import { Overview } from "./pages/Overview";
import { TypePage } from "./pages/TypePage";

/** The overview and one page per entity type the endpoint publishes. Add a page to `pages` for a new screen. */
export default function App() {
  const { config } = useClient();
  const { schema, error } = useSchema();
  const pages = useMemo((): Page[] => {
    if (!schema) return [];
    return [
      { id: "overview", label: "Overview", render: () => <Overview schema={schema} /> },
      ...Object.keys(schema)
        .sort()
        .map((type): Page => ({ id: type, label: type, render: () => <TypePage type={type} schema={schema[type]} /> })),
    ];
  }, [schema]);

  if (error) return <Problem error={error} />;
  if (!schema) return <Loading />;
  if (pages.length === 1) return <Empty>This endpoint publishes no entity types.</Empty>;
  return <AppShell title={config.appName ?? config.endpointName ?? "Application"} pages={pages} />;
}
