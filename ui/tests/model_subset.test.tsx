/** T-0575: a subset of the model is what an endpoint exposes (MP-01, MP-03, UI-43). */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { ModelSubsetPicker } from "../src/pages/models/ModelSubsetPicker";
import { parseModel } from "../src/pages/models/linkml";
import { EMPTY_SUBSET, subsetProblems, subsetSource, wholeSubset } from "../src/pages/models/subset";
import type { Subset } from "../src/pages/models/subset";

const SOURCE = `id: https://hel.fi/models/fleet
name: fleet
prefixes:
  hel: https://hel.fi/terms/
classes:
  Vehicle:
    class_uri: hel:Vehicle
    slots: [id, type, name, category, speed]
  User:
    class_uri: hel:User
    slots: [id, type, name, age]
slots:
  id:
    identifier: true
  type:
    designates_type: true
  name:
    range: string
  category:
    range: VehicleCategory
  speed:
    range: float
  age:
    range: integer
enums:
  VehicleCategory:
    permissible_values:
      public: {}
      private: {}
`;

const MODEL = parseModel(SOURCE);

function Harness({ initial = EMPTY_SUBSET }: { initial?: Subset }) {
  const [subset, setSubset] = useState<Subset>(initial);
  return (
    <>
      <ModelSubsetPicker model={MODEL} subset={subset} onChange={setSubset} />
      <pre aria-label="subset">{JSON.stringify(subset)}</pre>
    </>
  );
}

function renderPicker(initial?: Subset) {
  render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return {
    subset: () => JSON.parse(screen.getByLabelText("subset").textContent ?? "") as Subset,
    user: userEvent.setup(),
  };
}

describe("model subset", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("ticking Vehicle(id, name, type) and User(id, name, type, age) yields exactly that", async () => {
    const { subset, user } = renderPicker();
    expect(screen.getByRole("status")).toHaveTextContent("Nothing is ticked");

    await user.click(screen.getByRole("checkbox", { name: "Vehicle.name" }));
    await user.click(screen.getByRole("checkbox", { name: "User" }));
    await user.click(screen.getByRole("checkbox", { name: "User.name" }));
    await user.click(screen.getByRole("checkbox", { name: "User.age" }));

    expect(subset()).toEqual({
      classes: [
        { name: "Vehicle", slots: ["name"] },
        { name: "User", slots: ["name", "age"] },
      ],
    });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    // Ticking a slot ticked its class; the identity slots came with it and cannot go.
    expect(screen.getByRole("checkbox", { name: "Vehicle" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Vehicle.id" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Vehicle.id" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Vehicle.type" })).toBeDisabled();
  });

  it("distinguishes a class ticked with no slots from one not ticked", async () => {
    const { subset, user } = renderPicker(wholeSubset(MODEL));
    expect(subset().classes).toHaveLength(2);

    await user.click(screen.getByRole("checkbox", { name: "User.name" }));
    await user.click(screen.getByRole("checkbox", { name: "User.age" }));
    expect(subset().classes.find((klass) => klass.name === "User")).toEqual({ name: "User", slots: ["id", "type"] });

    await user.click(screen.getByRole("checkbox", { name: "User" }));
    expect(subset().classes.map((klass) => klass.name)).toEqual(["Vehicle"]);
    expect(screen.getByRole("checkbox", { name: "User.id" })).not.toBeChecked();
  });

  it("narrows the source to the chosen classes, slots and the enums they still use", () => {
    const narrowed = parseModel(
      subsetSource(SOURCE, {
        classes: [
          { name: "Vehicle", slots: ["name"] },
          { name: "User", slots: ["name", "age"] },
        ],
      }),
    );
    expect(narrowed.classes.map((klass) => [klass.name, klass.slots])).toEqual([
      ["Vehicle", ["id", "type", "name"]],
      ["User", ["id", "type", "name", "age"]],
    ]);
    expect(narrowed.slots.map((slot) => slot.name).sort()).toEqual(["age", "id", "name", "type"]);
    expect(narrowed.enums).toEqual([]);
    expect(narrowed.prefixes.hel).toBe("https://hel.fi/terms/");

    const withEnum = parseModel(subsetSource(SOURCE, { classes: [{ name: "Vehicle", slots: ["category"] }] }));
    expect(withEnum.classes).toHaveLength(1);
    expect(withEnum.enums.map((entry) => entry.name)).toEqual(["VehicleCategory"]);

    expect(parseModel(subsetSource(SOURCE, EMPTY_SUBSET)).classes).toEqual([]);
  });

  it("names every class and slot the model does not have", () => {
    expect(
      subsetProblems(MODEL, {
        classes: [
          { name: "Vehicle", slots: ["name", "colour"] },
          { name: "Bicycle", slots: ["id"] },
          { name: "Vehicle", slots: [] },
        ],
      }),
    ).toEqual([
      "class 'Vehicle' has no slot 'colour'",
      "unknown class 'Bicycle'",
      "class 'Vehicle' is listed twice",
    ]);
    render(
      <I18nextProvider i18n={i18n}>
        <ModelSubsetPicker
          model={MODEL}
          subset={{ classes: [{ name: "Bicycle", slots: [] }] }}
          onChange={() => {}}
        />
      </I18nextProvider>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("unknown class 'Bicycle'");
  });
});
