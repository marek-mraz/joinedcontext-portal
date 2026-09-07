import { describe, expect, it } from "vitest";
import type { RJSFSchema } from "@rjsf/utils";
import validator from "../src/components/forms/validator";

const schema: RJSFSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", pattern: "^[a-z0-9-]+$" },
    spec: {
      type: "object",
      properties: {
        period: { type: "string", minLength: 2 },
        kind: { type: "string", enum: ["bloblang", "wasm"] },
      },
    },
  },
};

describe("the Portal's schema validator", () => {
  it("reports a missing required property under the field, by ajv's keyword", () => {
    const { errors, errorSchema } = validator.validateFormData({}, schema);
    expect(errors.map((e) => e.name)).toEqual(["required"]);
    expect(errorSchema.name?.__errors).toHaveLength(1);
  });

  it("reports leaf errors with the field's path and drops the structural parents", () => {
    const { errors, errorSchema } = validator.validateFormData(
      { name: "Bad Name", spec: { period: "x", kind: "python" } },
      schema,
    );
    expect(errors.map((e) => [e.name, e.property]).sort()).toEqual([
      ["enum", ".spec.kind"],
      ["minLength", ".spec.period"],
      ["pattern", ".name"],
    ]);
    expect(errorSchema.spec?.kind?.__errors).toHaveLength(1);
    expect(errors.some((e) => e.name === "properties")).toBe(false);
  });

  it("passes valid data and answers isValid without a compiled function", () => {
    const original = globalThis.Function;
    // Under the Portal's CSP the browser refuses `new Function`; ajv dies here, this one must not.
    globalThis.Function = function () {
      throw new EvalError("blocked by CSP");
    } as unknown as FunctionConstructor;
    try {
      const { errors } = validator.validateFormData({ name: "ok", spec: { period: "5m", kind: "wasm" } }, schema);
      expect(errors).toEqual([]);
      expect(validator.isValid(schema, { name: "ok" }, schema)).toBe(true);
      expect(validator.isValid(schema, { name: 7 }, schema)).toBe(false);
    } finally {
      globalThis.Function = original;
    }
  });

  it("hands transformErrors the raw errors and keeps the custom messages", () => {
    const { errors } = validator.validateFormData({}, schema, undefined, (list) =>
      list.map((e) => ({ ...e, message: `translated ${e.name}` })),
    );
    expect(errors[0]?.message).toBe("translated required");
  });
});
