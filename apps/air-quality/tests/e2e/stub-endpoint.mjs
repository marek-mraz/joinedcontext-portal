// The Endpoint the app talks to during the browser flow: one space, one station, one
// grant that depends on the bearer token. It is deliberately not the gateway; what it
// reproduces is the only thing the flow needs, namely that the PDP answers differently for a
// steward and for a viewer, and that the app carries the caller's token to it (GW10).
import { createServer } from "node:http";

const PORT = Number(process.env.STUB_PORT ?? 4319);
const ID = "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:station-01";

let note = null;

const isSteward = (request) =>
  (request.headers.authorization ?? "").includes("steward");

const send = (response, status, body) => {
  if (body === undefined) {
    response.writeHead(status).end();
    return;
  }
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    "content-type": status >= 400 ? "application/problem+json" : "application/json",
    "content-length": Buffer.byteLength(payload),
  });
  response.end(payload);
};

const station = () => ({
  id: ID,
  type: "AirQualityObserved",
  name: { type: "Property", value: "Kallio" },
  pm10: { type: "Property", value: 34.2, observedAt: "2026-09-06T10:00:00Z" },
  pm25: { type: "Property", value: 21 },
  location: {
    type: "GeoProperty",
    value: { type: "Point", coordinates: [19.146, 48.736] },
  },
  ...(note ? { stewardNote: { type: "Property", value: note } } : {}),
});

createServer((request, response) => {
  const { pathname } = new URL(request.url, `http://127.0.0.1:${PORT}`);

  if (pathname === "/health") {
    return send(response, 200, { ok: true });
  }
  if (pathname === "/ngsi-ld/v1/entities" && request.method === "GET") {
    return send(response, 200, [station()]);
  }
  if (pathname === "/access/check" && request.method === "POST") {
    return send(response, 200, { decision: isSteward(request) });
  }
  if (pathname === `/ngsi-ld/v1/entities/${ID}/attrs` && request.method === "PATCH") {
    if (!isSteward(request)) {
      return send(response, 403, {
        type: "https://joinedcontext.com/errors/forbidden",
        title: "Forbidden",
        status: 403,
        detail: "writing stewardNote needs the project-steward role",
      });
    }
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      note = JSON.parse(body).stewardNote?.value ?? null;
      send(response, 204);
    });
    return undefined;
  }
  return send(response, 404, { status: 404, detail: `no route for ${pathname}` });
}).listen(PORT, "127.0.0.1", () => {
  process.stdout.write(`stub endpoint on http://127.0.0.1:${PORT}\n`);
});
