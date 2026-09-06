import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";

const STATION = "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:station-01";

const station = {
  id: STATION,
  name: "Kallio",
  pm10: 34.2,
  pm25: 21,
  observedAt: "2026-09-06T10:00:00Z",
  coordinates: [19.146, 48.736],
};

function serve(identity: Record<string, unknown>, writeStatus = 204, writeDetail?: string) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(status === 204 ? null : JSON.stringify(body), { status }));
    if (url.endsWith("api/me")) {
      return json(identity);
    }
    if (url.endsWith("api/stations")) {
      return json([station]);
    }
    if (init?.method === "POST") {
      return json(writeDetail ? { detail: writeDetail } : {}, writeStatus);
    }
    return json({}, 404);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

const steward = {
  signedIn: true,
  email: "demo.steward@hel.fi",
  user: "demo.steward@hel.fi",
  anonymous: false,
  canWriteNote: true,
};
const viewer = { ...steward, email: "demo.viewer@hel.fi", canWriteNote: false };
const nobody = { signedIn: false, email: null, user: null, anonymous: true, canWriteNote: false };

describe("air-quality app", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows the station metrics it was given and hides the ones it was not", async () => {
    serve(nobody);
    render(<App />);

    expect(await screen.findByText("Kallio")).toBeInTheDocument();
    expect(screen.getByText("34.2 µg/m³")).toBeInTheDocument();
    expect(screen.getByText("21 µg/m³")).toBeInTheDocument();
    // The grant returned no index, so there is no row for it rather than a zero.
    expect(screen.queryByText("Index")).toBeNull();
  });

  it("says who is signed in, and says so plainly when nobody is", async () => {
    serve(steward);
    render(<App />);
    expect(await screen.findByText(/demo.steward@hel.fi/)).toBeInTheDocument();
  });

  it("shows no note box to an anonymous reader", async () => {
    serve(nobody);
    render(<App />);
    await screen.findByText("Kallio");
    expect(screen.getByText("You are viewing anonymously.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save note" })).toBeNull();
  });

  it("shows no note box to a signed-in viewer the PDP refuses", async () => {
    serve(viewer);
    render(<App />);
    await screen.findByText("Kallio");
    expect(screen.queryByRole("button", { name: "Save note" })).toBeNull();
  });

  it("a steward writes a note and it goes to the app's own backend", async () => {
    const user = userEvent.setup();
    const fetchMock = serve(steward);
    render(<App />);
    await screen.findByText("Kallio");

    await user.type(screen.getByLabelText(/Steward note/), "Sensor cleaned.");
    await user.click(screen.getByRole("button", { name: "Save note" }));

    await waitFor(() => {
      const write = fetchMock.mock.calls.find((call) => call[1]?.method === "POST");
      expect(write).toBeDefined();
      expect(String(write?.[0])).toContain(`api/stations/${encodeURIComponent(STATION)}/note`);
      expect(JSON.parse(String(write?.[1]?.body))).toEqual({ note: "Sensor cleaned." });
    });
  });

  it("repeats the gateway's refusal instead of a generic failure", async () => {
    const user = userEvent.setup();
    serve(steward, 403, "writing stewardNote needs the project-steward role");
    render(<App />);
    await screen.findByText("Kallio");

    await user.type(screen.getByLabelText(/Steward note/), "Sensor cleaned.");
    await user.click(screen.getByRole("button", { name: "Save note" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "writing stewardNote needs the project-steward role",
    );
  });

  it("the save button stays disabled until there is something to save", async () => {
    serve(steward);
    render(<App />);
    await screen.findByText("Kallio");
    expect(screen.getByRole("button", { name: "Save note" })).toBeDisabled();
  });
});
