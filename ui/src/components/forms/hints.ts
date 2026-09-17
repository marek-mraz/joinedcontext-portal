/**
 * The pickers a schema asks for, as an RJSF `uiSchema` (CC-24, Development/05 section 2.1).
 *
 * JSON Schema can say that a value is a string; it cannot say that the strings worth choosing
 * are the spaces this project happens to hold. `x-jc-widget` on a property names the widget
 * that fills those in and `x-jc-options` carries what the widget needs. Everything else about
 * the form still comes from the schema alone, and a widget name the Portal does not register
 * is ignored: a parameter with its default input is a better answer than a form that throws.
 */

import type { JsonSchema, UiSchema } from "./types";
import { portalWidgets } from "./widgets";

/** What a property may carry beside the JSON Schema keywords. */
const WIDGET = "x-jc-widget";
const OPTIONS = "x-jc-options";

function properties(schema: unknown): Record<string, JsonSchema> | undefined {
  const declared = (schema as { properties?: unknown } | null)?.properties;
  return declared && typeof declared === "object"
    ? (declared as Record<string, JsonSchema>)
    : undefined;
}

/**
 * `shared` is merged under every picker's own options: the page renders the form and knows
 * the project, the schema does not and must not be able to name another one.
 */
export function pickers(
  schema: JsonSchema | undefined,
  shared: Record<string, unknown> = {},
): UiSchema {
  const declared = properties(schema);
  if (!declared) {
    return {};
  }

  const ui: UiSchema = {};
  for (const [name, property] of Object.entries(declared)) {
    const entry: Record<string, unknown> = {};
    const widget = (property as Record<string, unknown>)[WIDGET];
    if (typeof widget === "string" && widget in portalWidgets) {
      const own = (property as Record<string, unknown>)[OPTIONS];
      entry["ui:widget"] = widget;
      entry["ui:options"] = {
        ...shared,
        ...(own && typeof own === "object" ? (own as Record<string, unknown>) : {}),
      };
    }
    // An object parameter arranges its own properties under its name, the way RJSF nests them.
    Object.assign(entry, pickers(property, shared));
    if (Object.keys(entry).length > 0) {
      ui[name] = entry;
    }
  }
  return ui;
}
