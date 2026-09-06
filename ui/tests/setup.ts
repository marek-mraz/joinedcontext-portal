import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// Four agent sandboxes share the Mac's CPU and CI runs every file at once, so the
// library's 1 s default for findBy*/waitFor is not a margin, it is a coin flip.
configure({ asyncUtilTimeout: 5_000 });

if (typeof document !== "undefined") {
  document.documentElement.lang = "sk";
}

if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}
