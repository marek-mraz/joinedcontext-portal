import { describe, expect, it } from "vitest";
import { can, parseAccess } from "../src/sdk/access";
import type { AccessDocument } from "../src/sdk/access";

describe("sdk access", () => {
  it("answers checking reason when access is null", () => {
    const res = can(null, "queryEntity", "AirQualityObserved");
    expect(res).toEqual({ ok: false, reason: "Checking your permissions…" });
  });

  it("permits unconstrained access with wildcards", () => {
    const access: AccessDocument = {
      permissions: [{ resource: { type: "*" }, actions: ["*"], attributes: "*" }],
      prohibitions: [],
    };
    expect(can(access, "queryEntity", "Station")).toEqual({ ok: true });
    expect(can(access, "updateEntity", "Station", "bikes")).toEqual({ ok: true });
  });

  it("answers attribute-specific reason when operation is allowed but attribute is not", () => {
    const access: AccessDocument = {
      permissions: [
        {
          resource: { type: "Station" },
          actions: ["updateEntity"],
          attributes: ["status"],
        },
      ],
      prohibitions: [],
    };

    expect(can(access, "updateEntity", "Station", "status")).toEqual({ ok: true });
    expect(can(access, "updateEntity", "Station", "availableBikeNumber")).toEqual({
      ok: false,
      reason: "Your role may not change availableBikeNumber of Station.",
    });
    expect(can(access, "deleteEntity", "Station")).toEqual({
      ok: false,
      reason: "Your role may not deleteEntity Station.",
    });
  });

  it("prohibition overrides matching permission", () => {
    const access: AccessDocument = {
      permissions: [{ resource: { type: "Station" }, actions: ["*"], attributes: "*" }],
      prohibitions: [
        {
          resource: { type: "Station" },
          actions: ["deleteEntity"],
          attributes: "*",
        },
      ],
    };

    expect(can(access, "queryEntity", "Station")).toEqual({ ok: true });
    expect(can(access, "deleteEntity", "Station")).toEqual({
      ok: false,
      reason: "Your role may not deleteEntity Station.",
    });
  });

  it("parseAccess tolerates missing or malformed fields", () => {
    expect(parseAccess(null)).toEqual({ permissions: [], prohibitions: [] });
    expect(parseAccess({})).toEqual({ permissions: [], prohibitions: [] });
    expect(parseAccess({ permissions: "invalid" })).toEqual({ permissions: [], prohibitions: [] });
  });
});
