import { useMemo } from "react";
import { RouterProvider } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { AuthProvider, useAuth } from "./auth/AuthProvider";
import { BrandingProvider } from "./branding";
import { createPortalRouter } from "./router";

function RoutedApp({ router }: { router: ReturnType<typeof createPortalRouter> }) {
  const auth = useAuth();
  const { t } = useTranslation();

  // The route guards read `context.auth` in `beforeLoad`, which runs once per navigation.
  // Mounting the router before the session settles would let an anonymous visitor through the
  // guard on the very first match, so hold the tree until the answer is in.
  if (auth.status === "loading") {
    return (
      <p role="status" className="p-6 font-sans text-surface-fg">
        {t("app.loading")}
      </p>
    );
  }

  return <RouterProvider router={router} context={{ auth }} />;
}

export function App(): React.JSX.Element {
  const router = useMemo(() => createPortalRouter(), []);
  return (
    <BrandingProvider>
      <AuthProvider>
        <RoutedApp router={router} />
      </AuthProvider>
    </BrandingProvider>
  );
}

export default App;
