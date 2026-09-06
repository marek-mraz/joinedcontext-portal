import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema } from "../src/components/forms/types";

const schema: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: {
      type: "string",
      title: "Name",
    },
    count: {
      type: "number",
      title: "Count",
    },
  },
};

interface FormData {
  name: string;
  count?: number;
}

describe("SchemaForm", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("renders a schema with required string name and number count and asserts both labels exist", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<FormData> schema={schema} onSubmit={() => {}} />
      </I18nextProvider>
    );

    expect(screen.getByLabelText(/name/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/count/i)).toBeInTheDocument();
  });

  it("types 'hello' into name, submits, and asserts onSubmit received { name: 'hello' }", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();

    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<FormData> schema={schema} onSubmit={onSubmit} />
      </I18nextProvider>
    );

    const nameInput = screen.getByLabelText(/name/i);
    await user.type(nameInput, "hello");

    const submitBtn = screen.getByRole("button", { name: /submit/i });
    await user.click(submitBtn);

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith({ name: "hello" });
  });

  it("submits empty, asserts translated required message is shown, field carries aria-invalid='true', and onSubmit was not called", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();

    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<FormData> schema={schema} onSubmit={onSubmit} />
      </I18nextProvider>
    );

    const submitBtn = screen.getByRole("button", { name: /submit/i });
    await user.click(submitBtn);

    expect(screen.getByText("This field is required")).toBeInTheDocument();

    const nameInput = screen.getByLabelText(/name/i);
    expect(nameInput).toHaveAttribute("aria-invalid", "true");

    expect(onSubmit).not.toHaveBeenCalled();
  });
});
