/**
 * The Action Inspector (T-0607, AG-56, OPS-50): every tool step opens, a failed one goes back
 * into the run with one click, the trace is copied as JSON, and what the workspace sent is
 * text on the screen and nothing else (AG-46).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { errorText } from "../src/pages/apps/ActionStep";
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const EVENTS: RunEvent[] = [
  {
    seq: 1,
    kind: "tool",
    payload: {
      tool: "search_catalog",
      status: "ok",
      durationMs: 120,
      input: { q: "air quality" },
      output: { items: [{ name: "helsinki-air" }] },
    },
  },
  {
    seq: 2,
    kind: "tool",
    payload: {
      tool: "propose_endpoint",
      status: "failed",
      durationMs: 840,
      input: { name: "air-quality-public" },
      error: "422: representations must name at least one of ngsi-ld, geojson, csv",
      output: "<script>alert(1)</script>",
    },
  },
];

function renderPanel(live = true, events: RunEvent[] = EVENTS) {
  const onSend = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
        project="helsinki"
        events={events}
        streaming
        answering={false}
        sending={false}
        live={live}
        onAnswer={() => {}}
        onSend={onSend}
      />
    </I18nextProvider>,
  );
  return onSend;
}

describe("the action inspector", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows every step with its verdict and duration, and opens to the input and the output", async () => {
    renderPanel();
    const steps = screen.getAllByRole("group");
    expect(steps).toHaveLength(2);

    expect(within(steps[0]).getByRole("img", { name: en.agentRun.step.ok })).toBeInTheDocument();
    expect(within(steps[0]).getByText("search_catalog")).toBeInTheDocument();
    expect(within(steps[0]).getByText("120 ms")).toBeInTheDocument();

    await userEvent.click(within(steps[0]).getByText("search_catalog"));
    expect(within(steps[0]).getByText(en.agentRun.step.input)).toBeInTheDocument();
    expect(within(steps[0]).getByText(/"q": "air quality"/)).toBeInTheDocument();
    expect(within(steps[0]).getByText(/"name": "helsinki-air"/)).toBeInTheDocument();
  });

  it("sends a failed step back into the run with the error, only while the run is live", async () => {
    const onSend = renderPanel();
    const failed = screen.getAllByRole("group")[1];
    expect(within(failed).getByRole("img", { name: en.agentRun.step.failed })).toBeInTheDocument();

    await userEvent.click(within(failed).getByText("propose_endpoint"));
    await userEvent.click(within(failed).getByRole("button", { name: en.agentRun.step.fix }));
    expect(onSend).toHaveBeenCalledTimes(1);
    const text = onSend.mock.calls[0][0] as string;
    expect(text).toContain("propose_endpoint");
    expect(text).toContain("422: representations must name at least one of ngsi-ld, geojson, csv");
  });

  it("offers no fix once the run is over", () => {
    renderPanel(false);
    const failed = screen.getAllByRole("group")[1];
    expect(within(failed).queryByRole("button", { name: en.agentRun.step.fix })).toBeNull();
  });

  it("copies the step as JSON", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    renderPanel();
    const failed = screen.getAllByRole("group")[1];
    await userEvent.click(within(failed).getByText("propose_endpoint"));
    await userEvent.click(within(failed).getByRole("button", { name: en.agentRun.step.copy }));

    expect(writeText).toHaveBeenCalledTimes(1);
    const trace = JSON.parse(writeText.mock.calls[0][0] as string) as Record<string, unknown>;
    expect(trace.tool).toBe("propose_endpoint");
    expect(trace.status).toBe("failed");
    expect(trace.durationMs).toBe(840);
    expect(await within(failed).findByRole("button", { name: en.agentRun.step.copied })).toBeInTheDocument();
  });

  it("renders what the workspace sent as text, never as markup (AG-46)", async () => {
    renderPanel();
    const failed = screen.getAllByRole("group")[1];
    await userEvent.click(within(failed).getByText("propose_endpoint"));
    expect(within(failed).getByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });

  it("says on the step's own row why a function failed, with the file and line", async () => {
    const onSend = renderPanel(true, [
      {
        seq: 7,
        kind: "tool",
        payload: {
          tool: "function:summary",
          status: "failed",
          durationMs: 2,
          error: { file: "@joinedcontext/sdk/server", line: 97, message: "URLSearchParams is not defined" },
          output: { logs: [], status: 500 },
        },
      },
    ]);
    const step = screen.getByRole("group");
    const reason = within(step).getByTestId("step-reason");
    expect(reason).toHaveTextContent("URLSearchParams is not defined (@joinedcontext/sdk/server:97)");
    expect(reason).toHaveAttribute("title", "URLSearchParams is not defined (@joinedcontext/sdk/server:97)");

    await userEvent.click(within(step).getByText("function:summary"));
    await userEvent.click(within(step).getByRole("button", { name: en.agentRun.step.fix }));
    const text = onSend.mock.calls[0][0] as string;
    expect(text).toContain("function:summary");
    expect(text).toContain("URLSearchParams is not defined (@joinedcontext/sdk/server:97)");
  });

  it("counts the blocks a patch could not apply and names the first reason", async () => {
    renderPanel(true, [
      {
        seq: 9,
        kind: "tool",
        payload: {
          tool: "apply_patch",
          command: "4 block(s)",
          exitCode: 1,
          applied: [{ path: "src/pages/CustomHtml.tsx", how: "Created" }],
          refused: [
            { path: "src/components/ExportButton.tsx", reason: "SEARCH not found" },
            { path: "src/App.tsx", reason: "SEARCH not found" },
          ],
        },
      },
    ]);
    const step = screen.getByRole("group");
    expect(within(step).getByTestId("step-reason")).toHaveTextContent(
      "2 block(s) not applied: src/components/ExportButton.tsx: SEARCH not found",
    );
    await userEvent.click(within(step).getByText("apply_patch"));
    expect(within(step).getByText(en.agentRun.step.refused)).toBeInTheDocument();
  });

  it("shows no reason on a step that succeeded or failed without one", () => {
    const t = (key: string) => key;
    expect(errorText({ status: "ok" }, t)).toBe("");
    expect(errorText({ exitCode: 2 }, t)).toBe("");
    expect(errorText({ error: { message: "timeout" } }, t)).toBe("timeout");
    renderPanel(true, [EVENTS[0]]);
    expect(screen.queryByTestId("step-reason")).toBeNull();
  });
});
