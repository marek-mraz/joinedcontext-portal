import { useState } from "react";
import { useTranslation } from "react-i18next";
import { LanguageSwitcher } from "./components/LanguageSwitcher";
import { SchemaForm } from "./components/forms/SchemaForm";
import type { JsonSchema } from "./components/forms/types";

interface DemoAppConfig {
  name: string;
  replicas?: number;
}

const demoSchema: JsonSchema = {
  $schema: "http://json-schema.org/draft-07/schema#",
  type: "object",
  required: ["name"],
  properties: {
    name: {
      type: "string",
      title: "Name",
      minLength: 3,
    },
    replicas: {
      type: "number",
      title: "Replicas",
      minimum: 1,
    },
  },
};

export function App(): React.JSX.Element {
  const { t } = useTranslation();
  const [submittedData, setSubmittedData] = useState<DemoAppConfig | null>(null);

  return (
    <main className="min-h-screen bg-surface p-6 font-sans text-surface-fg">
      <header className="flex items-center justify-between border-b border-border pb-6">
        <h1 className="text-2xl font-bold">{t("app.title")}</h1>
        <LanguageSwitcher />
      </header>
      <section className="mt-6 max-w-lg">
        <SchemaForm<DemoAppConfig>
          schema={demoSchema}
          onSubmit={(data) => {
            setSubmittedData(data);
          }}
        />
        {submittedData && (
          <pre
            aria-label="Submitted Data"
            className="mt-4 rounded border border-border bg-surface-subtle p-4 font-mono text-sm"
          >
            {JSON.stringify(submittedData, null, 2)}
          </pre>
        )}
      </section>
    </main>
  );
}

export default App;
