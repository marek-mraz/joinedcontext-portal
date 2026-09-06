# syntax=docker/dockerfile:1
# One binary: the axum API, the embedded UI bundle and the in-process reconciler.

# The Vite build. rust-embed reads ui/dist at compile time, so the bundle has to exist
# before cargo runs; without this stage the binary ships the placeholder page and every
# view of DEMO.md is unreachable on the deployed platform (T-0369).
FROM node:24-slim AS ui
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
WORKDIR /ui
COPY ui/package.json ui/pnpm-lock.yaml ./
RUN corepack enable && pnpm install --frozen-lockfile
COPY ui/ ./
# An empty dist embeds as nothing and the placeholder page comes back silently, so the
# image build is where that is caught, not the demo.
RUN pnpm build && test -s dist/index.html && ls dist/assets/*.js >/dev/null

FROM rust:1.97-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY --from=ui /ui/dist ./ui/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && \
    install -m 0755 target/release/joinedcontext-portal /joinedcontext-portal

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /joinedcontext-portal /usr/local/bin/joinedcontext-portal
USER nonroot:nonroot
EXPOSE 8080
ENV JC_PORTAL_BIND=0.0.0.0:8080
ENTRYPOINT ["/usr/local/bin/joinedcontext-portal"]
