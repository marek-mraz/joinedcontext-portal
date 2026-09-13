/** T-0575: one editor, controlled from outside, in edit mode and in subset mode (DM-13, DM-31, MP-03). */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { LinkmlEditor } from "../src/pages/models/LinkmlEditor";
import { applyOperations } from "../src/pages/models/operations";
import { parseModel } from "../src/pages/models/linkml";
import type { Subset } from "../src/pages/models/subset";

const SOURCE = `id: https://hel.fi/models/fleet
name: fleet
prefixes:
  hel: https://hel.fi/terms/
classes:
  Vehicle:
    class_uri: hel:Vehicle
    slots: [id, name, speed]
  User:
    class_uri: hel:User
    slots: [id, name, age]
slots:
  id:
    identifier: true
  name:
    range: string
  speed:
    range: float
  age:
    range: integer
`;

/**
 * The page holds the document; the assistant button applies an operation to it the way an
 * assistant would, without touching the editor.
 */
function Harness({ subsetMode = false }: { subsetMode?: boolean }) {
  const [source, setSource] = useState(SOURCE);
  const [subset, setSubset] = useState<Subset>({ classes: [] });
  return (
    <>
      <button
        type="button"
        onClick={() =>
          setSource(
            applyOperations(source, [
              { op: "addSlot", name: "colour", class: "Vehicle", range: "string" },
            ]).source,
          )
        }
      >
        assistant adds colour
      </button>
      <LinkmlEditor
        source={source}
        onChange={setSource}
        locales={["en"]}
        {...(subsetMode ? { subset, onSubsetChange: setSubset } : {})}
      />
      <textarea readOnly aria-label="source" value={source} />
    </>
  );
}

function renderEditor(subsetMode = false) {
  const fetchMock = vi.fn((input: RequestInfo | URL) =>
    Promise.resolve(
      new Response(
        JSON.stringify({
          jsonSchema: { title: (input as Request).url },
          context: {},
          example: {},
          errors: [],
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Harness subsetMode={subsetMode} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return {
    fetchMock,
    source: () => (screen.getByLabelText("source") as HTMLTextAreaElement).value,
    user: userEvent.setup(),
  };
}

describe("LinkML editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is controlled: an operation applied outside the editor shows up in the tree", async () => {
    const { source, user } = renderEditor();
    expect(screen.getByRole("tab", { name: "Structure" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Subset" })).not.toBeInTheDocument();
    expect(screen.queryByRole("cell", { name: "colour" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "assistant adds colour" }));

    expect(await screen.findByRole("button", { name: "colour" })).toBeInTheDocument();
    expect(parseModel(source()).classes[0].slots).toEqual(["id", "name", "speed", "colour"]);

    // And a click in the tree edits the same string the assistant edited.
    await user.type(screen.getByLabelText("New slot"), "plate");
    await user.click(screen.getByRole("button", { name: "Add slot" }));
    expect(parseModel(source()).classes[0].slots).toEqual(["id", "name", "speed", "colour", "plate"]);
  });

  it("in subset mode shows the picker and previews the narrowed model", async () => {
    const { fetchMock, user } = renderEditor(true);
    expect(screen.getByRole("tab", { name: "Subset" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Structure" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "Vehicle.name" }));
    await user.click(screen.getByRole("tab", { name: "Preview" }));

    await screen.findByText(/Vehicle/);
    const request = fetchMock.mock.calls.at(-1)?.[0] as Request;
    expect(new URL(request.url).pathname).toBe("/api/v1/tools/generate");
    const compiled = parseModel(((await request.json()) as { source: string }).source);
    expect(compiled.classes.map((klass) => [klass.name, klass.slots])).toEqual([
      ["Vehicle", ["id", "name"]],
    ]);
    expect(compiled.slots.map((slot) => slot.name)).toEqual(["id", "name"]);
  });
});
