/** T-0222: browsing the Smart Data Models catalogue and importing one model (DM-07…DM-12). */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import {
  SmartDataModelsImport,
  deprecateUnused,
  matches,
} from "../src/pages/models/SmartDataModelsImport";
import { parseModel } from "../src/pages/models/linkml";
import en from "../src/locales/en.json";

const CATALOGUE = {
  refreshedAt: "2026-09-06T04:00:00Z",
  stale: false,
  subjects: [
    {
      name: "dataModel.Environment",
      title: "Environment",
      models: [
        {
          id: "dataModel.Environment/AirQualityObserved",
          name: "AirQualityObserved",
          description: "An observation of air quality conditions.",
          attributes: ["pm10", "pm25", "dateObserved"],
        },
        {
          id: "dataModel.Environment/NoiseLevelObserved",
          name: "NoiseLevelObserved",
          description: "A sound pressure observation.",
          attributes: ["LAeq"],
        },
      ],
    },
    {
      name: "dataModel.Transportation",
      title: "Transportation",
      models: [
        {
          id: "dataModel.Transportation/Vehicle",
          name: "Vehicle",
          description: "A vehicle of any kind.",
          attributes: ["speed", "location"],
        },
      ],
    },
  ],
};

const IMPORTED = `id: https://github.com/smart-data-models/dataModel.Environment/AirQualityObserved
name: AirQualityObserved
prefixes:
  sdm: https://smartdatamodels.org/
annotations:
  spec.source.repository: https://github.com/smart-data-models/dataModel.Environment
  spec.source.path: AirQualityObserved
  spec.source.commit: 9f1c2b7d4e6a8c0b2d4f6a8c0e2b4d6f8a0c2e4b
classes:
  AirQualityObserved:
    slots: [pm10, pm25, dateObserved]
slots:
  pm10:
    range: float
    slot_uri: sdm:pm10
  pm25:
    range: float
    slot_uri: sdm:pm25
  dateObserved:
    range: datetime
    slot_uri: sdm:dateObserved
`;

function renderWizard(options: { catalogue?: unknown; status?: number; spaces?: string[]; previewStatus?: number } = {}) {
  const onImport = vi.fn();
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    if (path === "/api/v1/tools/sdm-catalog") {
      return Promise.resolve(
        new Response(JSON.stringify(options.catalogue ?? CATALOGUE), {
          status: options.status ?? 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }
    return Promise.resolve(
      new Response(
        JSON.stringify(
          options.previewStatus && options.previewStatus >= 400
            ? { status: options.previewStatus, title: "Service Unavailable" }
            : { linkml: IMPORTED },
        ),
        {
          status: options.previewStatus ?? 200,
          headers: { "Content-Type": "application/json" },
        },
      ),
    );
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <SmartDataModelsImport onImport={onImport} spaces={options.spaces ?? []} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { onImport, fetchMock, user: userEvent.setup() };
}

describe("Smart Data Models import wizard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the catalogue by subject, through the Portal and not the catalogue itself", async () => {
    const { fetchMock } = renderWizard();

    expect(await screen.findByRole("button", { name: /AirQualityObserved/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Vehicle/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Environment" })).toBeInTheDocument();
    for (const call of fetchMock.mock.calls) {
      expect(new URL((call[0] as Request).url).origin).toBe(window.location.origin);
    }
  });

  it("searches by model name, description and attribute name", async () => {
    const { user } = renderWizard();
    await screen.findByRole("button", { name: /AirQualityObserved/ });

    await user.type(screen.getByLabelText("Search models and attributes"), "pm25");
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /Vehicle/ })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("button", { name: /AirQualityObserved/ })).toBeInTheDocument();

    expect(matches(CATALOGUE.subjects[1].models[0], "sound")).toBe(false);
    expect(matches(CATALOGUE.subjects[0].models[1], "sound")).toBe(true);
  });

  it("filters by subject", async () => {
    const { user } = renderWizard();
    await screen.findByRole("button", { name: /AirQualityObserved/ });

    await user.selectOptions(screen.getByLabelText("Subject"), "dataModel.Transportation");

    expect(await screen.findByRole("button", { name: /Vehicle/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /AirQualityObserved/ })).not.toBeInTheDocument();
  });

  it("previews the upstream model with the provenance of the commit it was pinned to", async () => {
    const { user, fetchMock } = renderWizard();

    await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));

    expect(await screen.findByText(/spec.source.commit/)).toBeInTheDocument();
    const post = fetchMock.mock.calls
      .map((call) => call[0] as Request)
      .find((request) => request.method === "POST");
    expect(new URL(post!.url).pathname).toBe("/api/v1/tools/import-sdm");
  });

  it("imports the model and keeps the slots the user did not pick, marked deprecated", async () => {
    const { user, onImport } = renderWizard();

    await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));
    await user.click(await screen.findByLabelText("pm25"));
    await user.click(screen.getByRole("button", { name: "Import AirQualityObserved" }));

    expect(onImport).toHaveBeenCalledTimes(1);
    const [source, model] = onImport.mock.calls[0];
    const imported = parseModel(source as string);
    expect(model).toMatchObject({ id: "dataModel.Environment/AirQualityObserved" });
    // DM-11: an unused upstream slot stays in the model so a partner's payload still validates.
    expect(imported.slots.map((slot) => slot.name)).toEqual(["pm10", "pm25", "dateObserved"]);
    expect(imported.slots.find((slot) => slot.name === "pm25")?.deprecated).toBe(true);
    expect(imported.slots.find((slot) => slot.name === "pm10")?.deprecated).toBe(false);
  });

  /// T-1109: Model Tools timing out is the ordinary failure here; picking the model again to
  /// try once more is a step nobody should have to know about.
  it("offers a retry when the import itself failed, and runs it", async () => {
    const { user, fetchMock } = renderWizard({ previewStatus: 503 });

    await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));
    const before = fetchMock.mock.calls.filter((call) =>
      new URL((call[0] as Request).url).pathname.includes("/tools/import-sdm"),
    ).length;
    expect(before).toBeGreaterThan(0);

    await user.click(await screen.findByRole("button", { name: en.app.error.retry }));
    await waitFor(() => {
      expect(
        fetchMock.mock.calls.filter((call) =>
          new URL((call[0] as Request).url).pathname.includes("/tools/import-sdm"),
        ).length,
      ).toBeGreaterThan(before);
    });
  });

  /// T-1107: a model of the catalogue can carry two hundred attributes.
  it("finds an attribute by name and puts the required ones first", async () => {
    const { user } = renderWizard();

    await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));
    await screen.findByLabelText("pm10");

    // The model's own order is pm10, pm25, dateObserved; required first regardless.
    const before = screen.getAllByRole("checkbox").map((box) => box.getAttribute("name"));
    expect(before.length).toBe(3);

    await user.type(screen.getByLabelText(en.models.sdm.findAttribute), "pm2");
    await waitFor(() => {
      expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    });
    expect(screen.getByLabelText(/pm25/)).toBeInTheDocument();

    await user.clear(screen.getByLabelText(en.models.sdm.findAttribute));
    await user.type(screen.getByLabelText(en.models.sdm.findAttribute), "nothing-of-the-sort");
    expect(await screen.findByText(en.models.sdm.noAttribute)).toBeInTheDocument();
  });

  /// T-1108, DM-57: the space a model belongs to, chosen where the model is chosen.
  it("carries the space the person picked into the import", async () => {
    const { user, onImport } = renderWizard({ spaces: ["mobility", "air-quality"] });

    await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));
    await user.selectOptions(await screen.findByLabelText(en.models.sdm.space), "air-quality");
    await user.click(screen.getByRole("button", { name: "Import AirQualityObserved" }));

    expect(onImport).toHaveBeenCalledTimes(1);
    expect(onImport.mock.calls[0][2]).toBe("air-quality");
  });

  it("leaves the space to the editor when the project has none to offer", async () => {
    const { user, onImport } = renderWizard();

    await user.click(await screen.findByRole("button", { name: /AirQualityObserved/ }));
    expect(screen.queryByLabelText(en.models.sdm.space)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Import AirQualityObserved" }));

    expect(onImport.mock.calls[0][2]).toBeUndefined();
  });

  it("works from the cached index when a refresh did not reach the catalogue", async () => {
    const { user, fetchMock } = renderWizard({
      catalogue: { ...CATALOGUE, stale: true },
    });

    expect(await screen.findByText(/Cached index from/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /AirQualityObserved/ })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Refresh now" }));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some((call) =>
          (call[0] as Request).url.includes("refresh=true"),
        ),
      ).toBe(true),
    );
  });

  it("says the catalogue is unreachable without blocking the editor", async () => {
    renderWizard({ catalogue: { title: "Service Unavailable" }, status: 503 });

    expect(await screen.findByText(/The catalogue is not reachable/)).toBeInTheDocument();
  });

  it("marks every unpicked slot and nothing else", () => {
    const marked = parseModel(deprecateUnused(IMPORTED, ["pm10"]));
    expect(marked.slots.filter((slot) => slot.deprecated).map((slot) => slot.name)).toEqual([
      "pm25",
      "dateObserved",
    ]);
  });
});
