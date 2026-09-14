export type { Cell, Row } from "../ngsi";
export type { DataClient, Query, TemporalPoint, TemporalQuery, TemporalRow } from "./client";
export { ProblemError } from "./client";
export type { JcUser } from "./config";
import type { JcUser } from "./config";

export interface FnRequest {
  method: "GET" | "POST";
  query: Record<string, string>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  body: any;
  user: JcUser | null;
}

export interface FnResponse {
  status?: number;
  body?: unknown;
}

export interface FnContext {
  jc: import("./client").DataClient;
  log(...parts: unknown[]): void;
}

export type FnHandler = (request: FnRequest, ctx: FnContext) => Promise<FnResponse>;
