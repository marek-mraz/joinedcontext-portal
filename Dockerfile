# syntax=docker/dockerfile:1
# One binary: the axum API, the embedded UI bundle and the in-process reconciler.
# The UI build stage is added together with ui/package.json; until then the binary serves
# the placeholder page and the API.

FROM rust:1.97-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
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
