import { describe, expect, it, vi } from "vitest";
import type { JcRequest, JcResponse } from "../src/sdk/transport";
import { endpointSource, spaceSource, fixtureSource, historyOf, sourceFor, SourceError } from "../src/grid/source";

function stubTransport(responses: Map<string, (req: JcRequest) => JcResponse>): (req: JcRequest) => Promise<JcResponse> {
  const calls: JcRequest[] = [];
  const fn = async (req: JcRequest): Promise<JcResponse> => {
    calls.push(req);
    const key = `${req.method} ${req.path}`;
    const handler = responses.get(key);
    if (handler) return handler(req);
    return { status: 404, body: { title: "Not Found" } };
  };
  fn.calls = calls;
  return fn as unknown as (req: JcRequest) => Promise<JcResponse> & { calls: JcRequest[] };
}

const sampleEntity: Record<string, unknown> = {
  id: "urn:ngsi-ld:Bike:hel:fi:001",
  type: "Bike",
  createdAt: "2025-01-01T00:00:00Z",
  modifiedAt: "2025-01-02T00:00:00Z",
  availableBikeNumber: {
    type: "Property",
    value: 5,
    unitCode: "C62",
    observedAt: "2025-01-01T12:00:00Z",
  },
  refDevice: {
    type: "Relationship",
    object: "urn:ngsi-ld:Device:hel:fi:d1",
  },
  location: {
    type: "GeoProperty",
    value: { type: "Point", coordinates: [24.95, 60.17] },
  },
};

const sampleEntity2: Record<string, unknown> = {
  id: "urn:ngsi-ld:Bike:hel:fi:002",
  type: "Bike",
  availableBikeNumber: {
    type: "Property",
    value: 3,
    unitCode: "C62",
  },
};

describe("endpointSource", () => {
  it("builds query URL with options=sysAttrs, type, limit, and no id in attrs", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({ status: 200, body: [sampleEntity] })],
    ]));
    const source = endpointSource("demo", transport);
    const result = await source.query({ type: "Bike" }, { offset: 0, limit: 50 });
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].id).toBe("urn:ngsi-ld:Bike:hel:fi:001");
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("options=sysAttrs");
    expect(call.path).toContain("type=Bike");
    expect(call.path).toContain("limit=50");
    // The footer shows how many entities the query matches, and only the broker can say
    // (T-1429): the grid holds one page and a page cannot be counted into a total.
    expect(call.path).toContain("count=true");
    expect(call.path).not.toContain("id");
  });

  it("escapes q parameter correctly", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50&q=speed%3E10%3Bbrand%3D%3D%22a%26b%22", () => ({ status: 200, body: [sampleEntity] })],
    ]));
    const source = endpointSource("demo", transport);
    await source.query({ type: "Bike", q: 'speed>10;brand=="a&b"' }, { offset: 0, limit: 50 });
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("q=speed%3E10%3Bbrand%3D%3D%22a%26b%22");
  });

  it("sends PATCH with JSON body and content type", async () => {
    const transport = stubTransport(new Map([
      ["PATCH /api/endpoint/demo/ngsi-ld/v1/entities/urn%3Angsi-ld%3ABike%3Ahel%3Afi%3A001/attrs", (req) => {
        expect(req.body).toEqual({ availableBikeNumber: { type: "Property", value: 10 } });
        return { status: 204, body: null };
      }],
    ]));
    const source = endpointSource("demo", transport);
    await source.patch!("urn:ngsi-ld:Bike:hel:fi:001", { availableBikeNumber: { type: "Property", value: 10 } });
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.method).toBe("PATCH");
    expect(call.body).toEqual({ availableBikeNumber: { type: "Property", value: 10 } });
  });

  it("throws SourceError with detail from problem body on 403", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({
        status: 403,
        body: { type: "about:blank", title: "Forbidden", detail: "You are not allowed to read this type" },
      })],
    ]));
    const source = endpointSource("demo", transport);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toThrow(SourceError);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toMatchObject({ status: 403, message: "You are not allowed to read this type" });
  });

  it("get returns null on 404", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities/urn%3Ax?options=sysAttrs", () => ({ status: 404, body: null })],
    ]));
    const source = endpointSource("demo", transport);
    const result = await source.get("urn:x");
    expect(result).toBeNull();
  });
});

describe("spaceSource", () => {
  it("uses /cs/{space}/ngsi-ld/v1/entities path", async () => {
    const transport = stubTransport(new Map([
      ["GET /cs/helsinki-bikes/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({ status: 200, body: [sampleEntity] })],
    ]));
    const source = spaceSource("helsinki-bikes", transport);
    const result = await source.query({ type: "Bike" }, { offset: 0, limit: 50 });
    expect(result.rows).toHaveLength(1);
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("/cs/helsinki-bikes/ngsi-ld/v1/entities");
  });

  it("throws SourceError with message 'no grant on this space' on 404", async () => {
    const transport = stubTransport(new Map([
      ["GET /cs/helsinki-bikes/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({ status: 404, body: null })],
    ]));
    const source = spaceSource("helsinki-bikes", transport);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toThrow(SourceError);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toMatchObject({ status: 404, message: "no grant on this space" });
  });

  it("get returns null on 404", async () => {
    const transport = stubTransport(new Map([
      ["GET /cs/helsinki-bikes/ngsi-ld/v1/entities/urn%3Ax?options=sysAttrs", () => ({ status: 404, body: null })],
    ]));
    const source = spaceSource("helsinki-bikes", transport);
    const result = await source.get("urn:x");
    expect(result).toBeNull();
  });
});

describe("fixtureSource", () => {
  const entities = [sampleEntity, sampleEntity2];

  it("filters by type and pages", async () => {
    const source = fixtureSource(entities);
    const page1 = await source.query({ type: "Bike" }, { offset: 0, limit: 1 });
    expect(page1.rows).toHaveLength(1);
    expect(page1.total).toBe(2);
    const page2 = await source.query({ type: "Bike" }, { offset: 1, limit: 1 });
    expect(page2.rows).toHaveLength(1);
    expect(page2.rows[0].id).toBe("urn:ngsi-ld:Bike:hel:fi:002");
  });

  it("filters by idPattern", async () => {
    const source = fixtureSource(entities);
    const result = await source.query({ type: "Bike", idPattern: "001$" }, { offset: 0, limit: 10 });
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].id).toBe("urn:ngsi-ld:Bike:hel:fi:001");
  });

  it("get returns entity by id", async () => {
    const source = fixtureSource(entities);
    const entity = await source.get("urn:ngsi-ld:Bike:hel:fi:001");
    expect(entity).not.toBeNull();
    expect(entity!.id).toBe("urn:ngsi-ld:Bike:hel:fi:001");
  });

  it("get returns null for unknown id", async () => {
    const source = fixtureSource(entities);
    const entity = await source.get("urn:unknown");
    expect(entity).toBeNull();
  });
});

describe("sourceFor", () => {
  it("returns endpointSource for endpoint kind", () => {
    const transport = vi.fn() as unknown as (req: JcRequest) => Promise<JcResponse>;
    const source = sourceFor({ kind: "endpoint", slug: "demo" }, transport);
    expect(source.query).toBeDefined();
    expect(source.get).toBeDefined();
  });

  it("returns spaceSource for space kind", () => {
    const transport = vi.fn() as unknown as (req: JcRequest) => Promise<JcResponse>;
    const source = sourceFor({ kind: "space", space: "helsinki" }, transport);
    expect(source.query).toBeDefined();
    expect(source.get).toBeDefined();
  });
});

describe("history", () => {
  it("asks between two instants and reads both temporal shapes", async () => {
    const seen: string[] = [];
    const transport = async (req: JcRequest): Promise<JcResponse> => {
      seen.push(req.path);
      return {
        status: 200,
        body: { speed: [{ type: "Property", value: 3, observedAt: "2026-01-01T00:00:00Z" }] },
      };
    };
    const points = await endpointSource("demo", transport).history!("urn:x", "speed", {
      from: "2026-01-01T00:00:00Z",
      to: "2026-01-02T00:00:00Z",
    });
    expect(seen[0]).toContain("/temporal/entities/urn%3Ax?attrs=speed&timerel=between");
    expect(seen[0]).toContain("endTimeAt=2026-01-02T00%3A00%3A00Z");
    expect(points).toEqual([{ at: "2026-01-01T00:00:00Z", value: 3 }]);
    expect(historyOf({ speed: { values: [[4, "2026-01-01T01:00:00Z"]] } }, "speed")).toEqual([
      { at: "2026-01-01T01:00:00Z", value: 4 },
    ]);
    expect(historyOf(null, "speed")).toEqual([]);
  });

  it("a space's history hides a missing grant like its query", async () => {
    const transport = async (): Promise<JcResponse> => ({ status: 404, body: { title: "Not Found" } });
    await expect(spaceSource("s", transport).history!("urn:x", "a", { lastN: 5 })).rejects.toMatchObject({
      status: 404,
      message: "no grant on this space",
    });
  });
});

describe("the count the footer shows (T-1429, R22)", () => {
  it("is the endpoint's own NGSILD-Results-Count", async () => {
    const transport = stubTransport(new Map([
      [
        "GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50",
        () => ({ status: 200, body: [sampleEntity], headers: { "ngsild-results-count": "412" } }),
      ],
    ]));
    const page = await endpointSource("demo", transport).query({ type: "Bike" }, { offset: 0, limit: 50 });
    expect(page.total).toBe(412);
  });

  it("is absent when the answer carries none, or carries something that is not a count", async () => {
    // A narrowed answer has its count removed on purpose: the difference between the count and
    // the rows is the number of entities the policy withheld (R22).
    const without = stubTransport(new Map([
      [
        "GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50",
        () => ({ status: 200, body: [sampleEntity] }),
      ],
    ]));
    expect((await endpointSource("demo", without).query({ type: "Bike" }, { offset: 0, limit: 50 })).total)
      .toBeUndefined();

    for (const raw of ["", "many", "-1", "1.5"]) {
      const odd = stubTransport(new Map([
        [
          "GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50",
          () => ({ status: 200, body: [sampleEntity], headers: { "ngsild-results-count": raw } }),
        ],
      ]));
      expect(
        (await endpointSource("demo", odd).query({ type: "Bike" }, { offset: 0, limit: 50 })).total,
        raw,
      ).toBeUndefined();
    }
  });
});
