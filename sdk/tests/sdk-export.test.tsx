import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Row } from "../src/ngsi";
import { ExportButton } from "../src/sdk/components/ExportButton";
import { JcProvider } from "../src/sdk/hooks";
import { stubClient } from "../src/sdk/testing";

const downloadMock = vi.fn();

vi.mock("../src/artifact", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/artifact")>();
  return {
    ...actual,
    download: (...args: unknown[]) => downloadMock(...args),
  };
});

describe("ExportButton component", () => {
  beforeEach(() => {
    downloadMock.mockClear();
  });

  const geoRows: Row[] = [
    {
      id: "urn:1",
      type: "Station",
      name: "Kamppi",
      location: { type: "Point", coordinates: [24.95, 60.15] },
    },
  ];

  const noGeoRows: Row[] = [
    {
      id: "urn:2",
      type: "Station",
      name: "Pasila",
      location: null,
    },
  ];

  it("calls download with appropriate filenames on CSV and GeoJSON clicks", () => {
    const client = stubClient();
    render(
      <JcProvider client={client}>
        <ExportButton rows={geoRows} filename="stations" formats={["csv", "geojson"]} />
      </JcProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "CSV" }));
    expect(downloadMock).toHaveBeenCalledWith(expect.any(Blob), "stations.csv");

    fireEvent.click(screen.getByRole("button", { name: "GeoJSON" }));
    expect(downloadMock).toHaveBeenCalledWith(expect.any(Blob), "stations.geojson");
  });

  it("disables GeoJSON button with notice when no geometry is present", () => {
    const client = stubClient();
    render(
      <JcProvider client={client}>
        <ExportButton rows={noGeoRows} formats={["geojson"]} />
      </JcProvider>,
    );

    const btn = screen.getByRole("button", { name: "GeoJSON" });
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute("title", "No geometry in these rows");
  });

  it("disables PNG button without canvas prop and enables it when provided", () => {
    const client = stubClient();
    const { rerender } = render(
      <JcProvider client={client}>
        <ExportButton rows={geoRows} formats={["png"]} />
      </JcProvider>,
    );

    expect(screen.getByRole("button", { name: "PNG" })).toBeDisabled();

    const canvasElem = document.createElement("canvas");
    rerender(
      <JcProvider client={client}>
        <ExportButton rows={geoRows} formats={["png"]} canvas={() => canvasElem} />
      </JcProvider>,
    );

    expect(screen.getByRole("button", { name: "PNG" })).not.toBeDisabled();
  });

  it("disables all buttons when rows are empty", () => {
    const client = stubClient();
    render(
      <JcProvider client={client}>
        <ExportButton rows={[]} formats={["csv", "geojson", "pdf", "png"]} />
      </JcProvider>,
    );

    expect(screen.getByRole("button", { name: "CSV" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "GeoJSON" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "PDF" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "PNG" })).toBeDisabled();
  });
});
