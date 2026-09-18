import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { PlanDiffViewer } from "../src/components/diff/PlanDiffViewer";
import type { FieldChange } from "../src/components/diff/PlanDiffViewer";

// `from`/`to` carry any JSON value; the generated schema types free-form JSON as an object.
const FIELDS = [
  { path: "spec.audience", to: "context-gateway" },
  { path: "spec.rateLimit.perMinute", from: 60, to: 600 },
  { path: "spec.representations[1]", from: "geojson" },
  { path: "spec.credentials.token", from: "[REDACTED]", to: "[REDACTED]" },
] as unknown as FieldChange[];

function renderDiff(fields?: FieldChange[] | null) {
  return render(
    <I18nextProvider i18n={i18n}>
      <PlanDiffViewer fields={fields} />
    </I18nextProvider>,
  );
}

function rowFor(path: string): HTMLElement {
  return screen.getByText(path).closest("tr") as HTMLElement;
}

describe("plan diff viewer", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("marks a field that only the desired state has as added", () => {
    renderDiff(FIELDS);
    const row = rowFor("spec.audience");
    expect(within(row).getByText(en.approvals.diffAdded)).toBeInTheDocument();
    expect(within(row).getByText("context-gateway")).toBeInTheDocument();
    expect(row.className).toContain("emerald");
  });

  it("shows before and after for a modified field", () => {
    renderDiff(FIELDS);
    const row = rowFor("spec.rateLimit.perMinute");
    expect(within(row).getByText(en.approvals.diffChanged)).toBeInTheDocument();
    expect(within(row).getByText("60")).toBeInTheDocument();
    expect(within(row).getByText("600")).toBeInTheDocument();
    expect(row.className).toContain("amber");
  });

  it("marks a field that only the current state has as removed", () => {
    renderDiff(FIELDS);
    const row = rowFor("spec.representations[1]");
    expect(within(row).getByText(en.approvals.diffRemoved)).toBeInTheDocument();
    expect(within(row).getByText("geojson")).toBeInTheDocument();
    expect(row.className).toContain("danger");
  });

  it("never prints a redacted value, on either side of the diff", () => {
    renderDiff(FIELDS);
    const row = rowFor("spec.credentials.token");
    expect(within(row).getAllByText(en.form.redacted)).toHaveLength(2);
    expect(row.textContent).not.toContain("[REDACTED]");
  });

  it("names each field in words, with its path underneath (T-1385)", () => {
    renderDiff([
      { path: "metadata.name", from: "old-name", to: "new-name" },
      { path: "spec.enabledRepresentations", from: ["ngsi-ld"], to: ["ngsi-ld", "geojson"] },
      { path: "spec.representations[1]", from: "geojson" },
    ] as unknown as FieldChange[]);
    const name = rowFor("metadata.name");
    expect(within(name).getByText("Name")).toBeInTheDocument();
    expect(within(name).getByText("old-name")).toBeInTheDocument();
    expect(within(name).getByText("new-name")).toBeInTheDocument();
    const list = rowFor("spec.enabledRepresentations");
    expect(within(list).getByText("Enabled representations")).toBeInTheDocument();
    expect(within(list).getByText("ngsi-ld, geojson")).toBeInTheDocument();
    expect(within(rowFor("spec.representations[1]")).getByText("Representations")).toBeInTheDocument();
  });

  it("folds a nested value away as its JSON, and shows an empty or null one as a dash", () => {
    renderDiff([
      { path: "spec.rateLimit", from: null, to: { perMinute: 600, burst: 20 } },
      { path: "spec.note", from: "", to: "x" },
    ] as unknown as FieldChange[]);
    const row = rowFor("spec.rateLimit");
    const folded = within(row).getByText("2 fields").closest("details") as HTMLElement;
    expect(folded).not.toHaveAttribute("open");
    expect(folded.querySelector("pre")?.textContent).toContain('"perMinute": 600');
    expect(within(row).getByText("—")).toBeInTheDocument();
    expect(within(rowFor("spec.note")).getByText("—")).toBeInTheDocument();
  });

  it("says so plainly when the plan changes no fields", () => {
    renderDiff([]);
    expect(screen.getByText(en.approvals.noChanges)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("labels the diff table for a screen reader", () => {
    renderDiff(FIELDS);
    expect(screen.getByRole("table", { name: en.approvals.diffType })).toBeInTheDocument();
  });
});
