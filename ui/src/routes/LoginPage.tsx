import { useTranslation } from "react-i18next";
import { useAuth } from "../auth/AuthProvider";
import { LanguageSwitcher } from "../components/LanguageSwitcher";
import { BrandMark } from "../components/layout/Shell";

export function LoginPage(): React.JSX.Element {
  const { t } = useTranslation();
  const { signIn } = useAuth();

  return (
    <main className="flex min-h-screen items-center justify-center bg-surface font-sans text-surface-fg">
      <div className="w-full max-w-sm rounded border border-border p-6">
        <div className="flex items-start justify-between gap-4">
          <h1 className="text-xl font-bold">
            <BrandMark />
          </h1>
          <LanguageSwitcher />
        </div>
        <button
          type="button"
          onClick={() => {
            signIn("/");
          }}
          className="mt-6 w-full rounded bg-primary px-4 py-2 text-sm font-medium text-primary-fg hover:opacity-90 focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
        >
          {t("auth.signIn")}
        </button>
      </div>
    </main>
  );
}
