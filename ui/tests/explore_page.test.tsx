import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { queryKeys } from "../src/api/client";
import { ExplorePage } from "../src/pages/explore/ExplorePage";
import en from "../src/locales/en.json";

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

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "helsinki", namespace: "helsinki" },
    spec: { dataModelRef: "helsinki-mobility" },
  },
]);

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-mobility", namespace: "helsinki" },
    spec: { classes: ["BikeHireDockingStation"], linkml: "models/helsinki-mobility.yaml" },
  },
]);

const ROW = { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001", type: "BikeHireDockingStation" };

/** The explorer with one endpoint, one model and one row, opened on that row's detail. */
async function openDetail(remove: () => Response, access?: unknown) {
  const calls: { method: string; url: string; csrf: string | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: Request) => {
      const request = input as Request;
      calls.push({
        method: request.method,
        url: request.url,
        csrf: request.headers.get("x-csrf-token"),
      });
      if (request.method === "DELETE") {
        return Promise.resolve(remove());
      }
      if (request.url.includes("/entities?")) {
        return Promise.resolve(
          new Response(JSON.stringify([ROW]), {
            status: 200,
            headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
          }),
        );
      }
      if (request.url.includes("/entities/")) {
        return Promise.resolve(
          new Response(JSON.stringify(ROW), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }
      if (access !== undefined && request.url.includes("/access")) {
        return Promise.resolve(
          new Response(JSON.stringify(access), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }
      // The model source and the access probe: nothing the delete needs.
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  await userEvent.selectOptions(await screen.findByLabelText(/Entity type/i), "BikeHireDockingStation");
  await userEvent.click(await screen.findByRole("button", { name: ROW.id }));
  await screen.findByTestId("explore-entity");
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer", () => {
  /// T-1116, UI-33: the page a person is looking at, as a file they can keep. It is written
  /// from the rows in hand, so the file is the view and not a second answer.
  it("writes the page it shows to a file", async () => {
    const captured: Blob[] = [];
    const revoked: string[] = [];
    let named = "";
    const urls = URL as unknown as Record<string, unknown>;
    const realCreate = urls.createObjectURL;
    const realRevoke = urls.revokeObjectURL;
    urls.createObjectURL = (blob: Blob) => {
      captured.push(blob);
      return "blob:the-page";
    };
    urls.revokeObjectURL = (href: string) => revoked.push(href);
    const clicked = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        named = this.download;
      });

    vi.stubGlobal(
      "fetch",
      vi.fn((input: Request) => {
        const request = input as Request;
        if (request.url.includes("/entities?")) {
          return Promise.resolve(
            new Response(JSON.stringify([ROW]), {
              status: 200,
              headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
            }),
          );
        }
        return Promise.resolve(new Response("", { status: 404 }));
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
    client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
    client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await userEvent.selectOptions(
      await screen.findByLabelText(/Entity type/i),
      "BikeHireDockingStation",
    );
    const button = await screen.findByRole("button", { name: en.explore.export });
    await waitFor(() => {
      expect(button).toBeEnabled();
    });
    await userEvent.click(button);

    expect(captured).toHaveLength(1);
    expect(captured[0].type).toBe("application/json");
    expect(JSON.parse(await captured[0].text())).toEqual([ROW]);
    expect(named).toBe("BikeHireDockingStation-1-1.json");
    expect(revoked).toEqual(["blob:the-page"]);

    clicked.mockRestore();
    urls.createObjectURL = realCreate;
    urls.revokeObjectURL = realRevoke;
  });

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

describe("removing an entity from the explorer (UI-60, T-0912)", () => {
  it("shows the endpoint's refusal and keeps the entity", async () => {
    await i18n.changeLanguage("en");
    const calls = await openDetail(
      () =>
        new Response(JSON.stringify({ detail: "the policy of this endpoint does not let you write BikeHireDockingStation" }), {
          status: 403,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    await userEvent.click(screen.getByTestId("explore-delete"));
    expect(within(screen.getByRole("dialog")).getByText(new RegExp(ROW.id))).toBeInTheDocument();
    await userEvent.click(screen.getByTestId("explore-delete-confirm"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the policy of this endpoint does not let you write BikeHireDockingStation",
    );
    // Still open on the entity: nothing was removed.
    expect(screen.getByTestId("explore-entity")).toBeInTheDocument();
    const deletes = calls.filter((call) => call.method === "DELETE");
    expect(deletes).toHaveLength(1);
    expect(deletes[0].url).toContain(
      `/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/ngsi-ld/v1/entities/${encodeURIComponent(ROW.id)}`,
    );
  });

  it("removes the entity through the endpoint that showed it and closes the detail", async () => {
    await i18n.changeLanguage("en");
    document.cookie = "jc_csrf=token-for-the-session";
    const calls = await openDetail(() => new Response(null, { status: 204 }));
    await userEvent.click(screen.getByTestId("explore-delete"));
    await userEvent.click(screen.getByTestId("explore-delete-confirm"));
    await waitFor(() => expect(screen.queryByTestId("explore-entity")).not.toBeInTheDocument());
    const deletes = calls.filter((call) => call.method === "DELETE");
    expect(deletes).toHaveLength(1);
    expect(deletes[0].csrf).toBe("token-for-the-session");
  });
});

/**
 * T-1021, UI-44 and EP-55: the endpoint's own grant decides whether this caller may remove an
 * entity. A grant that allows only reads leaves the button visible and disabled, with the
 * reason on it — never working until the gateway refuses it.
 */
it("disables the delete button, with a reason, when the grant allows only reads", async () => {
  await openDetail(
    () => new Response("", { status: 204 }),
    {
      subject: { type: "user", id: "demo.viewer" },
      permissions: [
        {
          resource: { type: "BikeHireDockingStation" },
          actions: ["queryEntity", "retrieveEntity"],
        },
      ],
    },
  );

  const remove = await screen.findByTestId("explore-delete");
  await waitFor(() => expect(remove).toBeDisabled());
  expect(remove).toHaveAttribute("title", expect.stringContaining("delete"));
});

/**
 * T-1017, UI-46: the assistant opens the explorer on the entity it found, so the detail is
 * already open when the person looks. Without it the assistant could only open the list and
 * say which row to click.
 */
it("opens on the entity the route names", async () => {
  const calls: { method: string; url: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: Request) => {
      const request = input as Request;
      calls.push({ method: request.method, url: request.url });
      const json = (body: unknown) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      if (request.url.includes("/entities/")) return json(ROW);
      if (request.url.includes("/entities?")) return json([ROW]);
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ExplorePage
          project="helsinki"
          initialSpace="helsinki"
          initialEndpoint="helsinki-bikes"
          initialEntityId={ROW.id}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );

  // The detail pane is open on that entity without anyone clicking a row.
  expect(await screen.findByTestId("explore-delete")).toBeInTheDocument();
  await waitFor(() =>
    expect(
      calls.some((call) => call.url.includes(`/entities/${encodeURIComponent(ROW.id)}`) || call.url.includes(`/entities/${ROW.id}`)),
    ).toBe(true),
  );
});
