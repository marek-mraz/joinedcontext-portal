import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { queryKeys } from "../src/api/client";
import { ExplorePage } from "../src/pages/explore/ExplorePage";

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-bikes", namespace: "helsinki", title: { en: "Helsinki city bike stations" } },
    spec: {
      contextSpaceRef: "helsinki",
      slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf",
      audience: "public",
      enabledRepresentations: ["ngsi-ld"],
    },
  },
]);

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer", () => {
  it("reads the endpoint list another page already cached as the API's List (T-0625)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify(list([])), { status: 200, headers: { "Content-Type": "application/json" } }),
        ),
      ),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    // What the app generator leaves in the cache under the same key.
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("option", { name: "Helsinki city bike stations" })).toBeInTheDocument();
  });
});
