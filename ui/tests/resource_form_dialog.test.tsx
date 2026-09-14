import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";

/**
 * T-0452: a `kind: UiSchema` manifest committed to `portal/forms/` changes the form in the
 * browser — order, widgets, help and grouping — with no code change (UI-01, UI-02, CC-29).
 */

const SCHEMA: JsonSchema = {
  type: "object",
  properties: {
    name: { type: "string", title: "Name" },
    slug: { type: "string", title: "Slug" },
    audience: { type: "string", title: "Audience" },
    commitMessage: { type: "string", title: "Commit message" },
  },
};

const ENDPOINT_FORM = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "UiSchema",
  metadata: { name: "endpoint", namespace: "org" },
  spec: {
    for: "Endpoint",
    order: ["audience", "name", "slug"],
    groups: [{ title: { en: "Basics", sk: "Základ" }, fields: ["name", "slug"] }],
    fields: {
      slug: { help: { en: "The unguessable address", sk: "Neuhádnuteľná adresa" } },
      commitMessage: { advanced: true },
    },
  },
};

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

function stubFetch(forms: unknown[], preferences: Record<string, unknown> = {}) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/preferences")) {
      return json(preferences);
    }
    if (path.endsWith("/forms")) {
      return json(list(forms));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function renderDialog(kind?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ResourceFormDialog
          open
          onOpenChange={() => {}}
          title="Endpoint"
          description="An endpoint"
          schema={SCHEMA}
          kind={kind}
          submitLabel="Propose"
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The visible field labels, in the order the form draws them. */
function labelsInOrder(): string[] {
  return screen
    .getAllByLabelText(/Name|Slug|Audience|Commit message/)
    .map((element) => element.getAttribute("id") ?? "");
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a manifest form", () => {
  it("draws the order and the grouping the manifest asks for", async () => {
    stubFetch([ENDPOINT_FORM]);
    renderDialog("Endpoint");

    // The fieldset the manifest's one group asks for, with its legend.
    const group = await screen.findByRole("group", { name: "Basics" });
    expect(within(group).getByLabelText(/Name/)).toBeTruthy();
    expect(within(group).getByLabelText(/Slug/)).toBeTruthy();

    // The help text of `slug` reaches the form, which is the thing FieldTemplate had to grow.
    expect(screen.getByText("The unguessable address")).toBeTruthy();

    // Grouping decides the layout: a grouped field is drawn inside its fieldset and what no
    // group names follows the fieldsets, whatever `order` says about the two together.
    const ids = labelsInOrder();
    expect(ids[0]).toContain("name");
    expect(ids[ids.length - 1]).toContain("audience");
  });

  it("draws the fields in the manifest's order when no group rearranges them", async () => {
    stubFetch([
      { ...ENDPOINT_FORM, spec: { ...ENDPOINT_FORM.spec, groups: [] } },
    ]);
    renderDialog("Endpoint");

    // `order` is [audience, name, slug] and the schema declares audience third.
    await waitFor(() => {
      expect(labelsInOrder()[0]).toContain("audience");
    });
    const ids = labelsInOrder();
    expect(ids[1]).toContain("name");
    expect(ids[2]).toContain("slug");
  });

  it("keeps an advanced field off the default form and brings it back with the toggle", async () => {
    stubFetch([ENDPOINT_FORM]);
    renderDialog("Endpoint");

    await screen.findByRole("group", { name: "Basics" });
    // CC-29: the commit message is declared, and simply not on the form.
    expect(screen.queryByLabelText(/Commit message/)).toBeNull();

    await userEvent.click(screen.getByLabelText("Show advanced fields"));
    await waitFor(() => {
      expect(screen.getByLabelText(/Commit message/)).toBeTruthy();
    });
  });

  it("renders exactly as before for a kind nobody has written a manifest for", async () => {
    stubFetch([ENDPOINT_FORM]);
    renderDialog("Pipeline");

    await waitFor(() => {
      expect(screen.getByLabelText(/Name/)).toBeTruthy();
    });
    // No manifest arranges a Pipeline here, so there is no group and nothing is hidden.
    expect(screen.queryByRole("group", { name: "Basics" })).toBeNull();
    expect(screen.getByLabelText(/Commit message/)).toBeTruthy();
  });

  it("says so when a manifest asks for something the form cannot do", async () => {
    stubFetch([
      {
        ...ENDPOINT_FORM,
        spec: { ...ENDPOINT_FORM.spec, fields: { nosuchfield: { widget: "text" } } },
      },
    ]);
    renderDialog("Endpoint");

    // A manifest silently ignored is the failure this whole path exists to avoid.
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("nosuchfield");
  });
});
