import i18n from "i18next";
import ICU from "i18next-icu";
import LanguageDetector from "i18next-browser-languagedetector";
import { initReactI18next } from "react-i18next";
import sk from "./locales/sk.json";
import en from "./locales/en.json";
import de from "./locales/de.json";
import cs from "./locales/cs.json";

export const SUPPORTED_LOCALES = ["sk", "en", "de", "cs"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];

void i18n
  .use(ICU)
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      sk: { translation: sk },
      en: { translation: en },
      de: { translation: de },
      cs: { translation: cs },
    },
    supportedLngs: SUPPORTED_LOCALES,
    fallbackLng: "sk",
    detection: {
      order: ["querystring", "localStorage", "navigator"],
      lookupQuerystring: "lang",
      lookupLocalStorage: "jc-lang",
      caches: ["localStorage"],
    },
    interpolation: {
      escapeValue: false,
    },
    returnNull: false,
  });

// WCAG 3.1.1: the document language has to follow the chosen locale, not stay at the
// `lang="sk"` baked into index.html.
i18n.on("languageChanged", (lng) => {
  if (typeof document !== "undefined") {
    document.documentElement.lang = lng;
  }
});

export default i18n;
