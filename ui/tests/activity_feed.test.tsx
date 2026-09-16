import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ActivityFeed } from "../src/components/ActivityFeed";
import { mergeActivity, objectOf, TAIL_WINDOW } from "../src/api/activity";
import { summarise } from "../src/components/ActivitySummary";
import type { ActivityEvent } from "../src/api/activity";

const PROJECT = "helsinki";

function event(overrides: Partial<ActivityEvent> = {}): ActivityEvent {
  return {
    time: "2026-09-16T16:21:03Z",
    project: PROJECT,
    space: "air-quality",
    kind: "access.denied",
    source: "gateway",
    summary: "An anonymous caller was refused a write.",
    severity: "warning",
    details: { object: "endpoints/public-air" },
    ...overrides,
  } as ActivityEvent;
}

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, ((event: MessageEvent) => void)[]> = {};

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(name: string, handler: (event: MessageEvent) => void) {
    (this.listeners[name] ||= []).push(handler);
  }

  removeEventListener(name: string, handler: (event: MessageEvent) => void) {
    this.listeners[name] = (this.listeners[name] || []).filter((h) => h !== handler);
  }

  close() {
    MockEventSource.instances = MockEventSource.instances.filter((i) => i !== this);
  }

  emit(name: string, data: unknown) {
    for (const handler of this.listeners[name] || []) {
      handler({ data: JSON.stringify(data) } as MessageEvent);
    }
  }
}

/** The rows the feed shows, head excluded; the skeleton renders none of them. */
async function rows() {
  return await waitFor(() => {
    const table = screen.getByRole("table");
    const found = within(table).getAllByRole("row").slice(1);
    expect(found.length).toBeGreaterThan(0);
    return found;
  });
}

function renderFeed(items: ActivityEvent[], props: Record<string, unknown> = {}) {
  const urls: string[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = input instanceof Request ? input.url : String(input);
    urls.push(url);
    const body = url.includes("/activity")
      ? { apiVersion: "joinedcontext.com/v1alpha1", kind: "ActivityList", items }
      : {};
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ActivityFeed project={PROJECT} {...props} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return urls;
}

describe("the activity feed (UI-31)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    MockEventSource.instances = [];
    vi.stubGlobal("EventSource", MockEventSource);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows what happened, in the order the API sent it, with the object it belongs to", async () => {
    renderFeed([
      event({ time: "2026-09-16T16:25:00Z", summary: "The newer one." }),
      event({ time: "2026-09-16T16:21:03Z", summary: "The older one." }),
    ]);

    const shown = await rows();
    expect(shown).toHaveLength(2);
    expect(shown[0]).toHaveTextContent("The newer one.");
    expect(within(shown[0]).getByRole("link", { name: "endpoints/public-air" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints/public-air",
    );
  });

  it("puts a filter into the request the list makes", async () => {
    const urls = renderFeed([event()]);
    await rows();

    await userEvent.selectOptions(
      screen.getByLabelText(en.activity.filter.severity),
      "error",
    );
    await waitFor(() =>
      expect(urls.some((url) => url.includes("severity=error"))).toBe(true),
    );
  });

  it("follows the tail and puts an arriving event at the top", async () => {
    renderFeed([event({ summary: "What the page loaded." })]);
    await rows();

    const source = MockEventSource.instances.at(-1);
    expect(source, "the tail is on by default").toBeDefined();
    source?.emit("pipeline.error", event({
      time: "2026-09-16T17:00:00Z",
      kind: "pipeline.error",
      severity: "error",
      summary: "A mapping dropped a reading.",
    }));

    await waitFor(() => {
      const shown = within(screen.getByRole("table")).getAllByRole("row").slice(1);
      expect(shown[0]).toHaveTextContent("A mapping dropped a reading.");
    });
    // Only an error interrupts a reader; the live region says nothing else.
    await waitFor(() =>
      expect(screen.getByTestId("activity-announce")).toHaveTextContent("1 new error"),
    );
  });

  it("stops following when the tail is switched off", async () => {
    renderFeed([event()]);
    await rows();
    expect(MockEventSource.instances).toHaveLength(1);

    await userEvent.click(screen.getByRole("switch", { name: en.activity.tail }));
    await waitFor(() => expect(MockEventSource.instances).toHaveLength(0));
  });

  it("an object panel offers no filters and no tail toggle", async () => {
    renderFeed([event()], { compact: true, fixed: { object: "endpoints/public-air" } });
    await rows();

    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.queryByLabelText(en.activity.filter.kind)).toBeNull();
  });

  it("says so when nothing has happened", async () => {
    renderFeed([]);
    expect(await screen.findByText(en.activity.empty)).toBeInTheDocument();
  });
});

describe("the tail's reducer", () => {
  it("keeps one copy of an event the page already showed", () => {
    const known = [event({ summary: "One." })];
    expect(mergeActivity(known, [event({ summary: "One." })])).toBe(known);
  });

  it("orders newest first and never grows past the window", () => {
    const many = Array.from({ length: TAIL_WINDOW + 20 }, (_, i) =>
      event({ time: `2026-09-16T${String(i % 24).padStart(2, "0")}:00:0${i % 10}Z`, summary: `n${i}` }),
    );
    const merged = mergeActivity([], many);
    expect(merged).toHaveLength(TAIL_WINDOW);
    expect(merged[0].time >= merged[1].time).toBe(true);
  });

  it("reads the object an event belongs to, and nothing that is not one", () => {
    expect(objectOf(event())).toBe("endpoints/public-air");
    expect(objectOf(event({ details: { object: "no-slash" } }))).toBeUndefined();
    expect(objectOf(event({ details: {} }))).toBeUndefined();
  });
});

describe("the last hour strip", () => {
  it("counts what an event counted, and one for an event that counted nothing", () => {
    expect(
      summarise([
        event({ kind: "pipeline.throughput", details: { count: 1200 } }),
        event({ kind: "pipeline.throughput", details: { count: 300 } }),
        event({ kind: "access.denied" }),
        event({ kind: "access.denied" }),
        event({ kind: "config.applied" }),
        event({ kind: "change.merged" }),
        event({ kind: "mcp.tool" }),
      ]),
    ).toEqual({ messages: 1500, changes: 2, requests: 0, forwards: 0, denials: 2 });
  });

  it("ignores a count that is not a number", () => {
    expect(
      summarise([event({ kind: "endpoint.traffic", details: { count: "many" } })]).requests,
    ).toBe(1);
  });
});
