# syntax=docker/dockerfile:1
# One binary: the axum API, the embedded UI bundle and the in-process reconciler.

# The Vite build. rust-embed reads ui/dist at compile time, so the bundle has to exist
# before cargo runs; without this stage the binary ships the placeholder page and every
# view of DEMO.md is unreachable on the deployed platform (T-0369).
FROM node:24-slim AS ui
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
# Both halves of the build context: `ui` depends on `@joinedcontext/sdk` as `link:../sdk` (T-1439,
# UI-71 — one grid, linked and never copied), so the sibling has to be beside it before
# `pnpm install` can resolve it, and its sources have to be there before Vite compiles them.
WORKDIR /work
COPY sdk/package.json sdk/pnpm-lock.yaml ./sdk/
COPY ui/package.json ui/pnpm-lock.yaml ./ui/
RUN corepack enable && cd ui && pnpm install --frozen-lockfile
COPY sdk/ ./sdk/
COPY ui/ ./ui/
WORKDIR /work/ui
# An empty dist embeds as nothing and the placeholder page comes back silently, so the
# image build is where that is caught, not the demo.
RUN pnpm build && test -s dist/index.html && ls dist/assets/*.js >/dev/null

# The App SDK (SDK-01) with the kit renderer the spec pass still fills in, embedded like the UI.
FROM node:24-slim AS sdk
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /sdk
COPY sdk/package.json sdk/pnpm-lock.yaml ./
RUN corepack enable && pnpm install --frozen-lockfile
COPY sdk/ ./
RUN pnpm build && test -s dist/kit.js && test -s dist/kit.css && test -s dist/kit-worker.js \
    && test -s dist/runtime/runtime.json && test -s dist/functions-server.js

FROM rust:1.97-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
# The repository is a cargo workspace whose members are the reference apps (AP-34). Cargo
# loads every member manifest before it builds anything, so `apps/` is a build input even
# though none of it reaches the image: without it the build dies on `failed to load
# manifest for workspace member /src/apps/*`.
COPY apps ./apps
# sqlx::migrate!("./migrations") reads the folder at COMPILE time, so it is a build input,
# not a runtime one: without it cargo fails with "error canonicalizing migration directory".
COPY migrations ./migrations
# Cargo.toml declares `[workspace] members = ["apps/*"]`, and cargo resolves every member's
# manifest before it builds anything, so without these the build dies on
# `failed to load manifest for workspace member /src/apps/*`. Their sources are needed too:
# a member's manifest is only valid if the targets it names exist.
COPY apps ./apps
COPY --from=ui /work/ui/dist ./ui/dist
COPY --from=sdk /sdk/dist ./sdk/dist
# The template every code run starts from is compiled into the binary too (src/agents/preview.rs).
COPY sdk/template ./sdk/template
# The kit's capabilities file is compiled into the binary (`include_str!` in
# src/agents/oneshot.rs, AP-65), so it is a build input of the Rust stage too.
COPY sdk/kit.json ./sdk/kit.json
# A code run's prompt carries the SDK's API and export list (`include_str!` in src/agents/code.rs).
COPY sdk/API.md ./sdk/API.md
COPY sdk/src/sdk/index.ts ./sdk/src/sdk/index.ts
# `-p joinedcontext-portal`: this image ships one binary and the reference apps have images of
# their own, so building the whole workspace here would compile them for nothing.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p joinedcontext-portal && \
    install -m 0755 target/release/joinedcontext-portal /joinedcontext-portal

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /joinedcontext-portal /usr/local/bin/joinedcontext-portal
USER nonroot:nonroot
EXPOSE 8080
ENV JC_PORTAL_BIND=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/joinedcontext-portal"]
