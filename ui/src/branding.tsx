import { createContext, useContext, useEffect, useMemo } from "react";
import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import i18n from "./i18n";
import { api, unwrap } from "./api/client";
import type { components } from "./api/schema";

export type Branding = components["schemas"]["Branding"];

/**
 * What an installation without a branding ConfigMap looks like. It mirrors the API's own
 * defaults so the first paint is never wrong: the answer only ever changes the values, never
 * the shape (UI-30).
 */
export const NEUTRAL_BRANDING: Branding = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "",
  organisation: "",
  orgDomain: "",
  domain: "",
  contactEmail: "",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const BrandingContext = createContext<Branding>(NEUTRAL_BRANDING);

/** The branding of this installation, applied and provided by {@link BrandingProvider}. */
export function useBranding(): Branding {
  return useContext(BrandingContext);
}

/** `/api/v1/branding/logo` when a logo is configured, nothing otherwise. */
export function logoUrl(branding: Branding): string | undefined {
  return branding.logo ? "/api/v1/branding/logo" : undefined;
}

/**
 * Writes the branding into the document: the title, and the colour and font tokens every
 * component already reads through Tailwind's theme.
 *
 * The values are custom properties on the root element, which is why the API validates each
 * colour as hex before it is served: a browser evaluates what lands here.
 */
export function applyBranding(branding: Branding, doc: Document = document): void {
  doc.title = branding.instanceName;
  const root = doc.documentElement;
  const tokens: Record<string, string | undefined> = {
    "--portal-color-primary": branding.colours?.primary,
    "--portal-color-primary-fg": branding.primaryForeground,
    "--portal-color-secondary": branding.colours?.secondary,
    "--portal-color-accent": branding.colours?.accent,
    "--portal-color-surface": branding.colours?.background,
    "--portal-color-surface-fg": branding.colours?.text,
    "--portal-font-sans": branding.fonts?.body,
    "--portal-font-heading": branding.fonts?.heading,
  };
  for (const [token, value] of Object.entries(tokens)) {
    if (value) {
      root.style.setProperty(token, value);
    }
  }
  const favicon = doc.querySelector<HTMLLinkElement>("link[rel~='icon']");
  if (favicon && branding.favicon) {
    favicon.href = "/api/v1/branding/favicon";
  }
}

/** The locales the switcher offers: what the installation configured, in its own order. */
export function offeredLocales(branding: Branding): string[] {
  const offered = branding.languages?.offered ?? [];
  return offered.length > 0 ? offered : NEUTRAL_BRANDING.languages!.offered!;
}

/**
 * Fetches the branding once, applies it, and hands it to the tree.
 *
 * The endpoint is public, so this runs before the session is known: the login page is
 * branded too. A failure is not an error state, it is the neutral look.
 */
export function BrandingProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const { data } = useQuery({
    queryKey: ["branding"],
    queryFn: async () => unwrap(await api.GET("/api/v1/branding")),
    staleTime: 5 * 60 * 1000,
  });
  // The answer is merged into the neutral block rather than replacing it: a field the API
  // did not send must not blank out a name or a colour the page needs.
  const branding: Branding = useMemo(() => ({ ...NEUTRAL_BRANDING, ...(data ?? {}) }), [data]);

  useEffect(() => {
    applyBranding(branding);
  }, [branding]);

  useEffect(() => {
    // The installation's default locale is a starting point, not a preference: a visitor who
    // has already chosen a language keeps it.
    const chosen =
      typeof window !== "undefined" ? window.localStorage.getItem("jc-lang") : null;
    const fallback = branding.languages?.default;
    if (!chosen && fallback && i18n.language !== fallback) {
      void i18n.changeLanguage(fallback);
    }
  }, [branding]);

  return <BrandingContext.Provider value={branding}>{children}</BrandingContext.Provider>;
}
