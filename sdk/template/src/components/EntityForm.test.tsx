import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Field, Row, Schema } from "@joinedcontext/sdk";
import { EntityForm, parseInput } from "./EntityForm";
import { stubClient } from "@joinedcontext/sdk/testing";

describe("parseInput", () => {
  it("parses every input kind and returns error on invalid input", () => {
    const textBase: Field = { name: "desc", input: "text", required: false };
    expect(parseInput(textBase, "")).toEqual({ value: null });
    expect(parseInput(textBase, "  ")).toEqual({ value: null });
    expect(parseInput(textBase, "Hello")).toEqual({ value: "Hello" });

    const textPattern: Field = { name: "code", input: "text", pattern: "[A-Z]{3}", required: false };
    expect(parseInput(textPattern, "ABC")).toEqual({ value: "ABC" });
    expect(parseInput(textPattern, "abc")).toEqual({ error: "does not match the expected format" });

    const numberField: Field = { name: "count", input: "number", min: 0, max: 100, required: false };
    expect(parseInput(numberField, "42")).toEqual({ value: 42 });
    expect(parseInput(numberField, "0")).toEqual({ value: 0 });
    expect(parseInput(numberField, "not-a-number")).toEqual({ error: "must be a number" });
    expect(parseInput(numberField, "-1")).toEqual({ error: "must be at least 0" });
    expect(parseInput(numberField, "101")).toEqual({ error: "must be at most 100" });

    const checkboxField: Field = { name: "active", input: "checkbox", required: false };
    expect(parseInput(checkboxField, "true")).toEqual({ value: true });
    expect(parseInput(checkboxField, "false")).toEqual({ value: false });

    const geoField: Field = { name: "location", input: "geo", required: false };
    expect(parseInput(geoField, "60.15, 24.95")).toEqual({
      value: { type: "Point", coordinates: [24.95, 60.15] },
    });
    expect(parseInput(geoField, "60.15 24.95")).toEqual({
      value: { type: "Point", coordinates: [24.95, 60.15] },
    });
    expect(parseInput(geoField, "invalid")).toEqual({ error: 'must be "lat, lon"' });
    expect(parseInput(geoField, "100, 200")).toEqual({ error: 'must be "lat, lon"' });

    const selectField: Field = { name: "status", input: "select", options: ["open", "closed"], required: false };
    expect(parseInput(selectField, "open")).toEqual({ value: "open" });
    expect(parseInput(selectField, "unknown")).toEqual({ error: "must be one of open, closed" });

    const dateField: Field = { name: "created", input: "date", required: false };
    expect(parseInput(dateField, "2025-05-14")).toEqual({ value: "2025-05-14" });
  });
});

describe("EntityForm component", () => {
  const schema: Schema = {
    Station: {
      properties: {
        name: { type: "string" },
        bikes: { type: "integer", minimum: 0 },
        status: { enum: ["open", "closed"] },
      },
      required: ["name"],
    },
  };

  it("create through stubClient writes the row and calls onSaved with minted urn", async () => {
    const client = stubClient({ schema });
    const onSaved = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" onSaved={onSaved} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("name")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("Local id"), { target: { value: "stat-001" } });
    fireEvent.change(screen.getByLabelText("name"), { target: { value: "Central Station" } });
    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "15" } });
    fireEvent.change(screen.getByLabelText("status"), { target: { value: "open" } });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSaved).toHaveBeenCalled();
    });

    const minted = onSaved.mock.calls[0][0];
    expect(minted).toBe("urn:ngsi-ld:Station:example.org:demo:stat-001");

    const rows = client.transport.rows();
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      id: "urn:ngsi-ld:Station:example.org:demo:stat-001",
      type: "Station",
      name: "Central Station",
      bikes: 15,
      status: "open",
    });
  });

  it("update sends only changed fields", async () => {
    const row: Row = {
      id: "urn:ngsi-ld:Station:001",
      type: "Station",
      name: "Central Station",
      bikes: 10,
      status: "open",
    };
    const client = stubClient({ schema, entities: [row] });
    const onSaved = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} onSaved={onSaved} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("bikes")).toHaveValue(10);
    });

    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "12" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onSaved).toHaveBeenCalledWith("urn:ngsi-ld:Station:001");
    });

    const patchCall = client.transport.calls.find((c) => c.method === "PATCH");
    expect(patchCall).toBeDefined();
    expect(patchCall?.body).toEqual({
      bikes: { type: "Property", value: 12 },
    });
  });

  it("required blank blocks the request and shows error", async () => {
    const client = stubClient({ schema });
    const onSaved = vi.fn();

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" onSaved={onSaved} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("name")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("name is required");
    expect(onSaved).not.toHaveBeenCalled();
    expect(client.transport.calls.filter((c) => c.method === "POST")).toHaveLength(0);
  });

  it("refuse fixture returning 403 shows detail with role alert and keeps typed text", async () => {
    const row: Row = {
      id: "urn:ngsi-ld:Station:001",
      type: "Station",
      name: "Central Station",
      bikes: 10,
    };
    const client = stubClient({
      schema,
      entities: [row],
      refuse: (req) => {
        if (req.method === "PATCH") {
          return {
            status: 403,
            body: { title: "Forbidden", detail: "You are not allowed to update this station." },
          };
        }
        return null;
      },
    });

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("bikes")).toBeInTheDocument();
    });

    fireEvent.change(screen.getByLabelText("bikes"), { target: { value: "99" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("You are not allowed to update this station.");
    expect(screen.getByLabelText("bikes")).toHaveValue(99);
  });

  it("disables a field with the reason when access document does not grant it", async () => {
    const row: Row = {
      id: "urn:ngsi-ld:Station:001",
      type: "Station",
      name: "Central Station",
      bikes: 10,
    };
    const client = stubClient({
      schema,
      entities: [row],
      access: {
        permissions: [
          {
            resource: { type: "Station" },
            actions: ["updateAttrs"],
            attributes: ["bikes"],
          },
        ],
        prohibitions: [],
      },
    });

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" row={row} />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("name")).toBeDisabled();
    });

    expect(screen.getByLabelText("name")).toHaveAttribute(
      "title",
      "Your role may not change name of Station.",
    );
    expect(screen.getByLabelText("bikes")).not.toBeDisabled();
  });

  it("renders a select input for schema enum", async () => {
    const client = stubClient({ schema });

    render(
      <JcProvider client={client}>
        <EntityForm type="Station" />
      </JcProvider>,
    );

    await waitFor(() => {
      expect(screen.getByLabelText("status")).toBeInTheDocument();
    });

    const select = screen.getByLabelText("status") as HTMLSelectElement;
    expect(select.tagName).toBe("SELECT");
    expect([...select.options].map((o) => o.value)).toEqual(["", "open", "closed"]);
  });
});
