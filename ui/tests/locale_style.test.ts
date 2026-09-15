/**
 * Plain copy (T-0755, UI-01, UI-45): the Portal speaks like a tool, not a chatbot. The English
 * bundle carries no first person, no filler adverbs or sales words, and no bundle shouts.
 */
import { describe, expect, it } from "vitest";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";

function strings(bundle: Record<string, unknown>, prefix = ""): [string, string][] {
  return Object.entries(bundle).flatMap(([key, value]): [string, string][] => {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") {
      return [[path, value]];
    }
    return value && typeof value === "object" ? strings(value as Record<string, unknown>, path) : [];
  });
}

const BANNED = [
  /\bI\b/,
  /\b(I'll|I've|I'm|I'd|let me|let's)\b/i,
  /\b(successfully|seamless(ly)?|powerful|effortless(ly)?|awesome|amazing|oops|sorry)\b/i,
];

describe("locale style", () => {
  it("keeps the English strings free of the assistant voice and filler", () => {
    const offending = strings(en).filter(([, text]) => BANNED.some((pattern) => pattern.test(text)));
    expect(offending).toEqual([]);
  });

  it.each([
    ["en", en],
    ["sk", sk],
    ["cs", cs],
    ["de", de],
  ])("puts no exclamation mark in the %s strings", (_, bundle) => {
    expect(strings(bundle).filter(([, text]) => text.includes("!"))).toEqual([]);
  });

  it.each([
    ["en", en],
    ["sk", sk],
    ["cs", cs],
    ["de", de],
  ])("writes every placeholder of the %s strings with the single braces ICU reads", (_, bundle) => {
    expect(strings(bundle).filter(([, text]) => /\{\{\w+\}\}/.test(text))).toEqual([]);
  });

  it("fails on the phrases it bans", () => {
    const bad = { a: { b: "I have stored it" }, c: "Let's continue where we left off.", d: "Saved successfully" };
    expect(strings(bad).filter(([, text]) => BANNED.some((pattern) => pattern.test(text)))).toHaveLength(3);
  });
});
