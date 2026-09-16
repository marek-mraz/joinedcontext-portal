import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { optionsOf, QuestionOptions } from "../src/pages/apps/QuestionOptions";
import type { JsonSchema } from "../src/components/forms/types";

const ENUM: JsonSchema = {
  type: "object",
  title: "Which context space?",
  properties: {
    answer: { type: "string", title: "Which context space?", enum: ["helsinki", "helsinki-kpi"] },
  },
  required: ["answer"],
} as JsonSchema;

const TEXT: JsonSchema = {
  type: "object",
  title: "What should it be called?",
  properties: { answer: { type: "string", title: "What should it be called?" } },
  required: ["answer"],
} as JsonSchema;

function show(schema: JsonSchema, onAnswer = vi.fn()) {
  render(
    <I18nextProvider i18n={i18n}>
      <QuestionOptions schema={schema} onAnswer={onAnswer} />
    </I18nextProvider>,
  );
  return onAnswer;
}

describe("a question of the assistant (UI-57)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("draws one button per option and answers with one click", async () => {
    const onAnswer = show(ENUM);

    expect(screen.getByRole("button", { name: "helsinki-kpi" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "helsinki" }));
    expect(onAnswer).toHaveBeenCalledWith({ answer: "helsinki" });
  });

  it("opens the text box for the answer nobody listed", async () => {
    show(ENUM);

    await userEvent.click(
      screen.getByRole("button", { name: en.agentRun.conversation.somethingElse }),
    );
    expect(screen.queryByTestId("question-options")).toBeNull();
    expect(screen.getByRole("button", { name: en.agentRun.conversation.answer })).toBeInTheDocument();
  });

  it("keeps the form for a question that is not a list", () => {
    show(TEXT);

    expect(screen.queryByTestId("question-options")).toBeNull();
    expect(screen.getByRole("button", { name: en.agentRun.conversation.answer })).toBeInTheDocument();
  });

  it("reads the options of an enum and of a oneOf of consts, and nothing else", () => {
    expect(optionsOf(ENUM)).toEqual(["helsinki", "helsinki-kpi"]);
    expect(
      optionsOf({
        type: "object",
        properties: { answer: { oneOf: [{ const: "yes" }, { const: "no" }] } },
      } as JsonSchema),
    ).toEqual(["yes", "no"]);
    expect(optionsOf(TEXT)).toBeNull();
    expect(optionsOf({ type: "object", properties: {} } as JsonSchema)).toBeNull();
  });
});
