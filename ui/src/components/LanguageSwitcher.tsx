import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useTranslation } from "react-i18next";
import { SUPPORTED_LOCALES } from "../i18n";
import type { Locale } from "../i18n";
import { offeredLocales, useBranding } from "../branding";

export function LanguageSwitcher(): React.JSX.Element {
  const { t, i18n } = useTranslation();
  const branding = useBranding();
  const resolved = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const currentLang = (SUPPORTED_LOCALES.includes(resolved as Locale) ? resolved : "sk") as Locale;
  // The installation says which languages it speaks; the bundle says which it can. A locale
  // the bundle has no strings for would switch the page to keys, so the list is the overlap,
  // and an installation that names none keeps every language the bundle carries.
  const offered = offeredLocales(branding).filter((locale): locale is Locale =>
    SUPPORTED_LOCALES.includes(locale as Locale),
  );
  const locales = offered.length > 0 ? offered : [...SUPPORTED_LOCALES];

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={t("lang.label")}
          className="inline-flex items-center justify-center rounded border border-border bg-surface px-3 py-1.5 text-sm font-medium text-surface-fg hover:bg-surface-subtle focus:outline-none focus:ring-2 focus:ring-border-focus focus:ring-offset-2"
        >
          {t(`lang.${currentLang}`)}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="z-50 min-w-[8rem] rounded border border-border bg-surface p-1 text-surface-fg shadow-md focus:outline-none"
        >
          {locales.map((locale) => {
            const isCurrent = locale === currentLang;
            return (
              <DropdownMenu.Item
                key={locale}
                aria-current={isCurrent ? "true" : undefined}
                onSelect={() => {
                  void i18n.changeLanguage(locale);
                }}
                className="relative flex cursor-pointer select-none items-center rounded px-2 py-1.5 text-sm text-surface-fg outline-none hover:bg-surface-subtle focus:bg-surface-subtle"
              >
                {t(`lang.${locale}`)}
              </DropdownMenu.Item>
            );
          })}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
