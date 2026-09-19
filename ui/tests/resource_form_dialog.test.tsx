import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom; a textarea
// with the same contract stands in, so the YAML view's own work is what runs here.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

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

function renderDialog(kind?: string, submitting?: boolean) {
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
          submitting={submitting}
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
  /// T-0962, UI-01: a disabled button is one that cannot be pressed for any reason. A person
  /// who clicked and sees nothing move clicks again, so the button says a proposal is in flight.
  it("says a proposal is in flight, and is not clickable while it is", async () => {
    stubFetch([]);
    renderDialog(undefined, true);

    const submit = await screen.findByRole("button", { name: "Propose" });
    expect(submit).toHaveAttribute("aria-busy", "true");
    expect(submit).toBeDisabled();
  });

  it("is an ordinary button when nothing is in flight", async () => {
    stubFetch([]);
    renderDialog();

    const submit = await screen.findByRole("button", { name: "Propose" });
    expect(submit).not.toHaveAttribute("aria-busy");
    expect(submit).toBeEnabled();
  });

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

  /**
   * UI-02, T-1608: the dialog opens with what the kind is for, taken from the kind's own
   * arrangement, and the shipped arrangements carry it for every kind with a form.
   */
  it("opens with what the kind is for, before any field", async () => {
    stubFetch([ENDPOINT_FORM]);
    renderDialog("Endpoint");

    const about = await screen.findByTestId("form-about");
    expect(about.textContent ?? "").toContain("endpoint is the door");
    // It stands above the fields, so it is read before anything is typed.
    const name = screen.getByLabelText(/^Name/);
    expect(about.compareDocumentPosition(name) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
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

/** The check a proposal needs is offered where the manifest was typed (T-0884, PF-57, AG-61). */
describe("the dialog's check", () => {
  type Form = { name: string; slug?: string; audience?: string };
  const source = {
    toManifest: (form: Form) => ({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: form.name },
      spec: { slug: form.slug, audience: form.audience },
    }),
    fromManifest: (document: unknown) => {
      const manifest = document as { metadata: { name: string }; spec: { slug?: string; audience?: string } };
      return { name: manifest.metadata.name, slug: manifest.spec.slug, audience: manifest.spec.audience };
    },
  };

  function renderWithCheck(onCheck: (form: Form) => void) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ResourceFormDialog<Form>
            open
            onOpenChange={() => {}}
            title="Endpoint"
            description="An endpoint"
            schema={SCHEMA}
            formData={{ name: "air", slug: "a", audience: "public" }}
            submitLabel="Propose"
            source={source}
            onCheck={onCheck}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  it("runs on the form in the form view", async () => {
    const onCheck = vi.fn();
    renderWithCheck(onCheck);
    await userEvent.click(screen.getByRole("button", { name: "Check" }));
    expect(onCheck).toHaveBeenCalledWith({ name: "air", slug: "a", audience: "public" });
  });

  it("is offered in the YAML view and runs on the manifest typed there", async () => {
    const onCheck = vi.fn();
    renderWithCheck(onCheck);
    await userEvent.click(screen.getByRole("tab", { name: "YAML" }));
    const editor = await screen.findByLabelText("YAML");
    fireEvent.change(editor, {
      target: {
        value: "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air\nspec:\n  slug: typed\n  audience: public\n",
      },
    });
    await userEvent.click(screen.getByRole("button", { name: "Check" }));
    expect(onCheck).toHaveBeenCalledWith({ name: "air", slug: "typed", audience: "public" });
  });

  it("says why when the YAML does not parse instead of checking anything", async () => {
    const onCheck = vi.fn();
    renderWithCheck(onCheck);
    await userEvent.click(screen.getByRole("tab", { name: "YAML" }));
    fireEvent.change(await screen.findByLabelText("YAML"), { target: { value: "metadata: [" } });
    await userEvent.click(screen.getByRole("button", { name: "Check" }));
    expect(onCheck).not.toHaveBeenCalled();
    expect(screen.getByText(/does not parse/)).toBeInTheDocument();
  });

  it("puts what the YAML answered after the editor and just before the buttons (T-1395)", async () => {
    renderWithCheck(vi.fn());
    await userEvent.click(screen.getByRole("tab", { name: "YAML" }));
    const editor = await screen.findByLabelText("YAML");
    fireEvent.change(editor, { target: { value: "metadata: [" } });
    await userEvent.click(screen.getByRole("button", { name: "Check" }));
    const alert = screen.getByText(/does not parse/).closest('[role="alert"]') as HTMLElement;
    const check = screen.getByRole("button", { name: "Check" });
    // DOCUMENT_POSITION_FOLLOWING: the alert comes after the editor, the button after the alert.
    expect(editor.compareDocumentPosition(alert) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(alert.compareDocumentPosition(check) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("is absent when the kind has no check", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ResourceFormDialog<Form>
            open
            onOpenChange={() => {}}
            title="Endpoint"
            description="An endpoint"
            schema={SCHEMA}
            formData={{ name: "air" }}
            submitLabel="Propose"
            source={source}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(screen.queryByRole("button", { name: "Check" })).not.toBeInTheDocument();
  });
});
