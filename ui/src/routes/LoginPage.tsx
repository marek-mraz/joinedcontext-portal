import { useTranslation } from "react-i18next";
import { useAuth } from "../auth/AuthProvider";
import { useBranding } from "../branding";
import { LanguageSwitcher } from "../components/LanguageSwitcher";
import { BrandMark } from "../components/layout/Shell";
import { Button, Icon } from "../components/ui";

/** Where to land after the login: the location the session ran out on, never another origin. */
export function redirectTarget(search: string): string {
  const candidate = new URLSearchParams(search).get("redirect_to");
  return candidate && candidate.startsWith("/") && !candidate.startsWith("//") ? candidate : "/";
}

export function LoginPage(): React.JSX.Element {
  const { t } = useTranslation();
  const { signIn } = useAuth();
  const branding = useBranding();

  return (
    <main className="relative flex min-h-screen items-center justify-center overflow-hidden bg-bg px-4 py-10 font-sans text-fg">
      {/* Two soft washes of the brand colour; decorative, and the same tokens as everything. */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -left-32 -top-32 size-[28rem] rounded-full bg-primary-100 blur-3xl"
      />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -bottom-40 -right-24 size-[24rem] rounded-full bg-primary-50 blur-3xl"
      />

      <div className="relative w-full max-w-sm">
        <div className="mb-4 flex justify-end">
          <LanguageSwitcher />
        </div>
        <div className="rounded-xl border border-border bg-surface p-8 shadow-2">
          <h1 className="text-title font-semibold">
            <BrandMark />
          </h1>
          <p className="mt-3 text-body text-fg-muted">{t("auth.lead")}</p>
          <Button
            variant="primary"
            size="lg"
            className="mt-8 w-full"
            icon={<Icon name="user" className="size-4" />}
            onClick={() => {
              signIn(redirectTarget(window.location.search));
            }}
          >
            {t("auth.signIn")}
          </Button>
        </div>
        {branding.organisation ? (
          <p className="mt-4 text-center text-caption text-fg-subtle">{branding.organisation}</p>
        ) : null}
      </div>
    </main>
  );
}
