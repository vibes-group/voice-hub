FROM node:24.22-bookworm-slim@sha256:be8204d4bd327363357f532187b752ff62759c31bedf98e9e18fc08c9cbd605c AS frontend

WORKDIR /src/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci

COPY frontend ./
RUN --mount=type=cache,target=/root/.npm npm run build

FROM rust:1.99-trixie@sha256:53766005847cbe02c3f59bfa49c1c4bb16695f2c9e5829c3d576a15406f5e733 AS chef

WORKDIR /src

# cook and build share this toolchain; the recipe does not carry it.
COPY backend/rust-toolchain.toml ./
RUN cargo install cargo-chef --version 0.1.78 --locked

FROM chef AS planner

COPY backend ./
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build

COPY --from=planner /src/recipe.json ./
RUN cargo chef cook --release --locked --bin voice-hub-app --recipe-path recipe.json

COPY backend ./
# Changes on every commit and lands in the CI layer cache: keep target out of it.
RUN cargo build --release --locked --bin voice-hub-app \
 && cp target/release/voice-hub-app /out-voice-hub-app \
 && rm -rf target

FROM debian:13-slim@sha256:89c1ea91e897f0fe0a3731595330bc45b8bdef4047464a07116f891a94054b31

RUN apt-get update \
 && apt-get upgrade -y \
 && apt-get install -y --no-install-recommends ca-certificates wget \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd -r app && useradd -r -g app -u 10001 -d /app -s /sbin/nologin app

WORKDIR /app

COPY --from=build /out-voice-hub-app /app/voice-hub-app
COPY --from=frontend /src/frontend/dist /app/web

# Persistent state (connection-password hash). Mounted as a volume in compose.
RUN mkdir -p /app/data && chown app:app /app/data

ENV APP_ADDR=:8080
ENV APP_WEB_DIR=/app/web
ENV APP_UPDATES_DIR=/app/updates

EXPOSE 8080

USER app:app

CMD ["/app/voice-hub-app"]
