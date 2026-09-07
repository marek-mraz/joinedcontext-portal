import { useTranslation } from "react-i18next";
import { SUPPORTED_LOCALES } from "../i18n";
import type { Locale } from "../i18n";
import { offeredLocales, useBranding } from "../branding";
import { Icon, Menu, MenuContent, MenuItem, MenuTrigger, buttonClass } from "./ui";

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
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          aria-label={t("lang.label")}
          className={buttonClass("ghost", "sm")}
        >
          <Icon name="globe" className="size-4 text-fg-muted" />
          <span>{t(`lang.${currentLang}`)}</span>
          <Icon name="chevronDown" className="size-3.5 text-fg-subtle" />
        </button>
      </MenuTrigger>
      <MenuContent align="end" className="min-w-[9rem]">
        {locales.map((locale) => {
          const isCurrent = locale === currentLang;
          return (
            <MenuItem
              key={locale}
              aria-current={isCurrent ? "true" : undefined}
              onSelect={() => {
                void i18n.changeLanguage(locale);
              }}
            >
              {isCurrent ? <Icon name="check" className="size-4" /> : <span className="size-4" />}
              {t(`lang.${locale}`)}
            </MenuItem>
          );
        })}
      </MenuContent>
    </Menu>
  );
}
