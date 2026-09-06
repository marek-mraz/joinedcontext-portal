# air-quality

The reference app of AP-34: the stations of one Context Space, their latest values, a day of
history and a note box for the people the gateway lets write. Architecture in
[Apps on Demand §6](https://github.com/marek-mraz/joinedcontext-docs/blob/main/Architecture/16-apps-on-demand.md).

It reaches exactly one thing, the Endpoint whose URL it is handed, and it holds no credential
of its own. The login in front of it is the oauth2-proxy sidecar, which forwards the user's
access token; the app carries that token to the endpoint and shows whatever comes back,
refusal included.

## Run it

```bash
JC_ENDPOINT_URL=https://{host}/api/endpoint/{slug}/ \
JC_BASE_PATH=/apps/air-quality/ \
JC_BIND_ADDRESS=127.0.0.1:8080 \
cargo run -p air-quality
```

`JC_ENDPOINT_URL` is the only required variable. Build the UI first with `pnpm install &&
pnpm build` in `ui/`; a debug build reads `ui/dist` from disk, a release build embeds it.

## Test it

```bash
cargo test -p air-quality                 # the backend and its contract with the endpoint
cd ui && pnpm test                        # the screens
cd ui && pnpm exec playwright test        # the steward and viewer flows against the binary
```

The browser flow starts the real binary against `tests/e2e/stub-endpoint.mjs` and plays the
sidecar itself, setting the forwarded headers oauth2-proxy would set.
