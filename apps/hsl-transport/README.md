# hsl-transport

The public reference app (AP-34, AP-38). Thirty buses on a map, moving.

It reads one Context Space through one Endpoint and nothing else: no broker, no database, no
second host, no credential. The Endpoint it is bound to is public, so the app never sees a
user and asks nobody to sign in (AP-28). Its sibling `air-quality` covers the other half of
the login front, with a Keycloak session and a write.

## How it stays live without a streaming endpoint

An Endpoint has no subscription surface and this app does not add one. One poll loop reads
`ngsi-ld/v1/entities` as GeoJSON every two seconds, keeps the last known position of each bus
in memory, and pushes what moved to every open browser as Server-Sent Events on
`{JC_BASE_PATH}api/stream`. A hundred open maps are still one query per interval, and the
browser never learns the Endpoint URL.

A poll that fails leaves the last known fleet on every map: the buses stop moving rather than
disappearing, which is the honest thing to show.

## Environment

| Variable | Meaning |
|---|---|
| `JC_ENDPOINT_URL` | the Endpoint this app reads, the only host it calls. Required |
| `JC_BASE_PATH` | the path it is served under, `/apps/hsl-transport/`. Defaults to `/` |
| `JC_POLL_SECONDS` | seconds between two polls. Defaults to 2 |
| `JC_BIND_ADDRESS` | defaults to `127.0.0.1:8080`, because the sidecar is the only entrance |

## Running it

```sh
cd ui && pnpm install && pnpm build && cd ..
JC_ENDPOINT_URL=https://<host>/api/endpoint/<slug>/ cargo run
```

## Tests

```sh
cargo test -p hsl-transport      # the contract with the Endpoint, against a stub
cd ui && pnpm test               # the map source, against a MapLibre double
```

The Rust tests assert what actually leaves the pod: the representation asked for, the thirty
bus cap, that no credential is ever attached, and that serving a browser costs no query. The
UI tests assert that a bus moving on the Endpoint reaches the one GeoJSON source the map
draws from, without the layer being rebuilt. MapLibre needs WebGL, which jsdom has not, so
the library is replaced by a double; there is no Playwright flow here for the same reason.
