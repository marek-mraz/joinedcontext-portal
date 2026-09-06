import type { RJSFSchema, UiSchema } from "@rjsf/utils";

export type { RJSFSchema as JsonSchema, UiSchema };

export interface FormChange<T> {
  formData?: T;
}
