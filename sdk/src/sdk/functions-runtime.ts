// The module jc-functions loads as `@joinedcontext/sdk/server`: the server entry, plus the client
// the runtime builds `ctx.jc` from over its one host request (Architecture/20 §3).
export * from "./server";
export { createClient } from "./client";
