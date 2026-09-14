import { describe, expect, it } from "vitest";
import { createClient, isEndpointPath, ProblemError } from "../src/sdk/client";
import type { JcConfig } from "../src/sdk/config";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";

const CONFIG: JcConfig = {
  slug: "demo",
  orgDomain: "example.org",
  space: "demo",
  transport: "bridge",
  language: "fi",
};

describe("isEndpointPath validation", () => {
  it("accepts exactly the seven allowed endpoint path shapes", () => {
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Ax%3A1")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Ax%3A1/attrs")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/temporal/entities")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/schema/index.json")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/schema/v1/json-schema")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/schema/v42/json-schema")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/access")).toBe(true);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&limit=10")).toBe(true);
  });

  it("refuses unauthorized paths, path traversal, multiple slashes, backslashes, and other slugs", () => {
    expect(isEndpointPath("demo", "/api/endpoint/other/ngsi-ld/v1/entities")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/subscriptions")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities/../access")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo//ngsi-ld/v1/entities")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/ngsi-ld/v1/entities\\attrs")).toBe(false);
    expect(isEndpointPath("demo", "/api/endpoint/demo/access#section")).toBe(false);
  });
});

describe("DataClient entity operations", () => {
  it("entities.all pages 1000 at a time, stops on short page, and caps at 5000", async () => {
    const calls: JcRequest[] = [];
    const scriptedTransport: Transport = async (req: JcRequest): Promise<JcResponse> => {
      calls.push(req);
      const url = new URL(req.path, "http://localhost");
      const offset = Number(url.searchParams.get("offset")) || 0;
      const limit = Number(url.searchParams.get("limit")) || 1000;
      const count = offset >= 2000 ? 500 : limit; // short page at 2000
      const page = Array.from({ length: count }, (_, i) => ({
        id: `urn:ngsi-ld:T:${offset + i}`,
        type: "T",
        n: offset + i,
      }));
      return { status: 200, body: page };
    };

    const client = createClient(CONFIG, scriptedTransport);
    const rows = await client.entities.all("T", { limit: 5000 });

    expect(rows).toHaveLength(2500);
    expect(calls).toHaveLength(3); // 0..1000, 1000..2000, 2000..2500
  });

  it("entities.list sends keyValues, strips id and type from attrs, and uses config.language", async () => {
    let sentPath = "";
    const transport: Transport = async (req) => {
      sentPath = req.path;
      return {
        status: 200,
        body: [
          {
            id: "urn:1",
            type: "Station",
            name: { languageMap: { fi: "Kamppi", en: "Kamppi EN" } },
          },
        ],
      };
    };

    const client = createClient(CONFIG, transport);
    const rows = await client.entities.list("Station", { attrs: ["id", "type", "name"] });

    expect(sentPath).toContain("attrs=name");
    expect(sentPath).not.toContain("attrs=id");
    expect(sentPath).toContain("options=keyValues");
    expect(rows[0].name).toBe("Kamppi"); // picked languageMap.fi
  });

  it("throws ProblemError with detail when endpoint refuses (e.g. 403)", async () => {
    const transport: Transport = async () => ({
      status: 403,
      body: { title: "Forbidden", detail: "Role cannot read this entity" },
    });

    const client = createClient(CONFIG, transport);
    await expect(client.entities.list("Secret")).rejects.toThrowError(ProblemError);
    await expect(client.entities.list("Secret")).rejects.toMatchObject({
      status: 403,
      detail: "Role cannot read this entity",
    });
  });

  it("throws ProblemError when list response is not an array", async () => {
    const transport: Transport = async () => ({ status: 200, body: { not: "an array" } });
    const client = createClient(CONFIG, transport);
    await expect(client.entities.list("T")).rejects.toThrowError("The endpoint did not answer a list.");
  });

  it("create mints URN, encodes GeoProperty and Property, drops nulls, and validates type/localId", async () => {
    let postedBody: unknown;
    const transport: Transport = async (req) => {
      postedBody = req.body;
      return { status: 201, body: null };
    };

    const client = createClient(CONFIG, transport);

    // Bad localId refused before sending
    await expect(client.entities.create("T", {}, "invalid id with spaces")).rejects.toThrow(ProblemError);
    // Bad type refused before sending
    await expect(client.entities.create("bad-type!", {})).rejects.toThrow(ProblemError);

    const id = await client.entities.create(
      "Station",
      {
        name: "Kamppi",
        bikes: 10,
        empty: null,
        location: { type: "Point", coordinates: [24.9, 60.1] },
      },
      "001",
    );

    expect(id).toBe("urn:ngsi-ld:Station:example.org:demo:001");
    expect(postedBody).toEqual({
      id: "urn:ngsi-ld:Station:example.org:demo:001",
      type: "Station",
      name: { type: "Property", value: "Kamppi" },
      bikes: { type: "Property", value: 10 },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.9, 60.1] } },
    });
  });

  it("update sends PATCH and empty patch sends nothing", async () => {
    const calls: JcRequest[] = [];
    const transport: Transport = async (req) => {
      calls.push(req);
      return { status: 204, body: null };
    };

    const client = createClient(CONFIG, transport);
    await client.entities.update("urn:ngsi-ld:T:1", {});
    expect(calls).toHaveLength(0); // empty patch skips request

    await client.entities.update("urn:ngsi-ld:T:1", { bikes: 5 });
    expect(calls).toHaveLength(1);
    expect(calls[0].method).toBe("PATCH");
    expect(calls[0].path).toBe("/api/endpoint/demo/ngsi-ld/v1/entities/urn%3Angsi-ld%3AT%3A1/attrs");
    expect(calls[0].body).toEqual({ bikes: { type: "Property", value: 5 } });
  });

  it("remove sends DELETE to entity path", async () => {
    let deletedPath = "";
    const transport: Transport = async (req) => {
      deletedPath = req.path;
      return { status: 204, body: null };
    };
    const client = createClient(CONFIG, transport);
    await client.entities.remove("urn:1");
    expect(deletedPath).toBe("/api/endpoint/demo/ngsi-ld/v1/entities/urn%3A1");
  });
});

describe("temporal, schema, and functions", () => {
  it("temporal.list requires endTimeAt for between, sends temporalValues, and parses response", async () => {
    let reqPath = "";
    const transport: Transport = async (req) => {
      reqPath = req.path;
      return {
        status: 200,
        body: [
          {
            id: "urn:1",
            type: "T",
            speed: {
              type: "Property",
              values: [
                [10, "2026-09-12T10:00:00Z"],
                { value: 12, observedAt: "2026-09-12T11:00:00Z" },
              ],
            },
          },
        ],
      };
    };

    const client = createClient(CONFIG, transport);
    await expect(
      client.temporal.list("T", { timerel: "between", timeAt: "2026-09-12T00:00:00Z" }),
    ).rejects.toThrow("endTimeAt is required");

    const rows = await client.temporal.list("T", {
      timerel: "after",
      timeAt: "2026-09-12T00:00:00Z",
    });

    expect(reqPath).toContain("options=temporalValues");
    expect(rows).toHaveLength(1);
    expect(rows[0].series.speed).toEqual([
      { value: 10, observedAt: "2026-09-12T10:00:00Z" },
      { value: 12, observedAt: "2026-09-12T11:00:00Z" },
    ]);
  });

  it("schema resolves version from index.json, caches success, and retries on failure", async () => {
    let count = 0;
    const transport: Transport = async (req) => {
      if (req.path.endsWith("index.json")) {
        count++;
        if (count === 1) return { status: 500, body: { title: "Server error" } };
        return { status: 200, body: { models: [{ version: 2 }, { version: 3 }] } };
      }
      if (req.path.endsWith("/v2/json-schema")) return { status: 200, body: { definitions: { Station: { properties: {} } } } };
      return { status: 200, body: { $defs: { Bike: { properties: {} } } } };
    };

    const client = createClient(CONFIG, transport);
    await expect(client.schema()).rejects.toThrow();

    const s1 = await client.schema();
    expect(s1).toHaveProperty("Station");
    expect(s1).toHaveProperty("Bike");

    // Second call should hit the cache (no more requests)
    const s2 = await client.schema();
    expect(s2).toBe(s1);
  });

  it("functions.call routes to /functions/{name} on bridge and /apps/{appName}/... on origin", async () => {
    const calls: JcRequest[] = [];
    const transport: Transport = async (req) => {
      calls.push(req);
      if (req.path.includes("runtime-err")) {
        return {
          status: 500,
          body: { error: { message: "Division by zero", file: "math.ts", line: 42 } },
        };
      }
      return { status: 200, body: { result: "ok" } };
    };

    const bridgeClient = createClient({ ...CONFIG, transport: "bridge" }, transport);
    const originClient = createClient({ ...CONFIG, transport: "origin", appName: "my-app" }, transport);

    await expect(bridgeClient.functions.call("BadName!")).rejects.toThrow("Invalid function name");

    const r1 = await bridgeClient.functions.call("rebalance", { factor: 2 });
    expect(calls[0].path).toBe("/functions/rebalance");
    expect(r1).toEqual({ result: "ok" });

    await originClient.functions.call("notify", { to: "user" });
    expect(calls[1].path).toBe("/apps/my-app/api/functions/notify");

    try {
      await bridgeClient.functions.call("runtime-err");
      expect.unreachable("should have thrown");
    } catch (err) {
      expect(err).toBeInstanceOf(ProblemError);
      const pe = err as ProblemError;
      expect(pe.message).toBe("Division by zero");
      expect(pe.file).toBe("math.ts");
      expect(pe.line).toBe(42);
    }
  });
});
