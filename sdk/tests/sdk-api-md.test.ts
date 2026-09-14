import { describe, expect, it } from "vitest";
import apiMd from "../API.md?raw";
import * as sdk from "../src/sdk/index";
import * as server from "../src/sdk/server";
import * as testing from "../src/sdk/testing";

// SDK-09: API.md is what the model reads, so an export it does not list is one the model never uses.
describe("API.md", () => {
  it.each([
    ["@joinedcontext/sdk", sdk],
    ["@joinedcontext/sdk/server", server],
    ["@joinedcontext/sdk/testing", testing],
  ])("lists every runtime export of %s", (_entry, exports) => {
    const missing = Object.keys(exports).filter((name) => !new RegExp(`\\b${name}\\b`).test(apiMd));
    expect(missing).toEqual([]);
  });

  it("stays within 12 000 tokens (4 characters per token, rounded up)", () => {
    expect(Math.ceil(apiMd.length / 4)).toBeLessThanOrEqual(12_000);
  });
});
