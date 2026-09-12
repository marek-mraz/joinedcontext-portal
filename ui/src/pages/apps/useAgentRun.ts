import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, unwrap } from "../../api/client";

/** The states of Architecture/19 §5, in the order a run walks them (AG-43). */
export const RUN_STATES = [
  "queued",
  "starting",
  "interviewing",
  "building",
  "testing",
  "previewing",
  "awaiting_approval",
  "published",
] as const;
export type RunState = (typeof RUN_STATES)[number];

/** A run that ended, whichever way. Nothing more arrives on its stream. */
export const TERMINAL_STATES = ["published", "failed", "cancelled", "expired"];

/**
 * The event kinds the stream carries (API/04 §4). Every frame is a named event, so the hook
 * subscribes to each kind by name: `onmessage` alone would see none of them.
 */
export const EVENT_KINDS = [
  "status",
  "question",
  "answer",
  "message",
  "thought",
  "tool",
  "commit",
  "preview",
  "usage",
  "lag",
] as const;

export interface AgentRun {
  id: string;
  project: string;
  appName: string;
  endpointName: string;
  appClass: string;
  visibility: string;
  prompt: string;
  status: string;
  steps: number;
  tokensUsed: number;
  previewUrl?: string;
  createdBy: string;
  createdAt: string;
  error?: string;
  ticket?: string;
}

export interface RunEvent {
  seq: number;
  kind: string;
  payload: Record<string, unknown>;
}

/** One question the agent is waiting on, as the `question` event carries it. */
export interface RunQuestion {
  questionId: string;
  schema: Record<string, unknown>;
  required?: boolean;
}

export function questionOf(event: RunEvent): RunQuestion | null {
  const id = event.payload.questionId;
  const schema = event.payload.schema;
  if (typeof id !== "string" || typeof schema !== "object" || schema === null) {
    return null;
  }
  return { questionId: id, schema: schema as Record<string, unknown>, required: true };
}

/** The questions nobody has answered yet, oldest first. */
export function openQuestions(events: RunEvent[]): RunQuestion[] {
  const answered = new Set(
    events
      .filter((event) => event.kind === "answer")
      .map((event) => String(event.payload.questionId ?? "")),
  );
  return events
    .filter((event) => event.kind === "question")
    .map(questionOf)
    .filter((question): question is RunQuestion => question !== null)
    .filter((question) => !answered.has(question.questionId));
}

/**
 * One run, live: the record, its event stream, and the three things a person may do to it.
 *
 * The stream is the browser's own `EventSource`, which is what makes the resume free: it
 * reconnects on its own and sends `Last-Event-ID`, and the Portal replays from there (AG-45).
 * The record is re-read whenever a `status` event arrives rather than polled, so a run that
 * says nothing costs nothing.
 */
export function useAgentRun(project: string, runId: string | null) {
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [streaming, setStreaming] = useState(false);
  const queryClient = useQueryClient();
  const key = ["agent-run", project, runId];
  const seen = useRef(new Set<number>());

  const run = useQuery({
    queryKey: key,
    enabled: runId !== null,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs/{id}", {
          params: { path: { project, id: runId ?? "" } },
        }),
      ) as unknown as AgentRun,
  });

  useEffect(() => {
    if (runId === null) {
      return;
    }
    // Resolved at call time: jsdom has no EventSource, and a test substitutes one.
    const Source = (globalThis as { EventSource?: typeof EventSource }).EventSource;
    if (!Source) {
      return;
    }
    const source = new Source(
      `/api/v1/projects/${encodeURIComponent(project)}/agent-runs/${encodeURIComponent(runId)}/events`,
      { withCredentials: true },
    );
    const onOpen = () => {
      setStreaming(true);
    };
    const onError = () => {
      setStreaming(false);
    };
    const handlers = EVENT_KINDS.map((kind) => {
      const handler = (message: MessageEvent<string>) => {
        let payload: Record<string, unknown> = {};
        try {
          payload = JSON.parse(message.data) as Record<string, unknown>;
        } catch {
          // A frame the Portal did not write. Dropping it beats rendering half of it.
          return;
        }
        const seq = Number(payload.seq ?? message.lastEventId ?? 0);
        // The replay a reconnect brings overlaps what the stream already showed.
        if (seen.current.has(seq)) {
          return;
        }
        seen.current.add(seq);
        setEvents((current) => [...current, { seq, kind, payload }].sort((a, b) => a.seq - b.seq));
        if (kind === "status" || kind === "preview" || kind === "usage") {
          void queryClient.invalidateQueries({ queryKey: key });
        }
      };
      source.addEventListener(kind, handler as EventListener);
      return { kind, handler };
    });
    source.addEventListener("open", onOpen);
    source.addEventListener("error", onError);

    return () => {
      for (const { kind, handler } of handlers) {
        source.removeEventListener(kind, handler as EventListener);
      }
      source.removeEventListener("open", onOpen);
      source.removeEventListener("error", onError);
      source.close();
      setStreaming(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, runId]);

  const answer = useMutation({
    mutationFn: async (body: { questionId: string; answers: unknown }) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/answers", {
          params: { path: { project, id: runId ?? "" } },
          body: body as never,
        }),
      ),
  });

  const send = useMutation({
    mutationFn: async (text: string) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/messages", {
          params: { path: { project, id: runId ?? "" } },
          body: { text },
        }),
      ),
  });

  const cancel = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/cancel", {
          params: { path: { project, id: runId ?? "" } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  const publish = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs/{id}/publish", {
          params: { path: { project, id: runId ?? "" } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: key }),
  });

  const reset = useCallback(() => {
    seen.current = new Set<number>();
    setEvents([]);
  }, []);

  return { run, events, streaming, answer, send, cancel, publish, reset };
}
