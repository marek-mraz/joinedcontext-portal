import type { JSX } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { SchemaForm } from "../../components/forms/SchemaForm";
import type { JsonSchema } from "../../components/forms/types";
import { Button } from "../../components/ui";

/** The options of a question whose answer is one of a list, or `null` when it is free text. */
export function optionsOf(schema: JsonSchema | undefined): string[] | null {
  const answer = (schema as { properties?: Record<string, unknown> } | undefined)?.properties
    ?.answer as { enum?: unknown; oneOf?: unknown } | undefined;
  if (!answer) {
    return null;
  }
  if (Array.isArray(answer.enum)) {
    const options = answer.enum.filter((value): value is string => typeof value === "string");
    return options.length > 0 ? options : null;
  }
  if (Array.isArray(answer.oneOf)) {
    const options = answer.oneOf
      .map((one) => (one as { const?: unknown }).const)
      .filter((value): value is string => typeof value === "string");
    return options.length > 0 ? options : null;
  }
  return null;
}

/**
 * One question of the assistant (UI-57). A single enum is a row of buttons — one click answers
 * it — with "Something else…" for the answer nobody listed; anything else keeps the form.
 */
export function QuestionOptions({
  schema,
  disabled,
  onAnswer,
}: {
  schema: JsonSchema;
  disabled?: boolean;
  onAnswer: (data: unknown) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [typing, setTyping] = useState(false);
  const options = optionsOf(schema);

  if (!options || typing) {
    return (
      <SchemaForm
        schema={schema}
        disabled={disabled}
        submitLabel={t("agentRun.conversation.answer")}
        onSubmit={onAnswer}
      />
    );
  }

  return (
    <div className="flex flex-wrap gap-2" data-testid="question-options">
      {options.map((option) => (
        <Button
          key={option}
          size="sm"
          variant="secondary"
          disabled={disabled}
          onClick={() => onAnswer({ answer: option })}
        >
          {option}
        </Button>
      ))}
      <Button
        size="sm"
        variant="ghost"
        disabled={disabled}
        onClick={() => setTyping(true)}
      >
        {t("agentRun.conversation.somethingElse")}
      </Button>
    </div>
  );
}
