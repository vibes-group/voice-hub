FROM node:24.22-bookworm-slim@sha256:a5f4366e834695d5f363b920c104953a535d3a9864d1bdf74566a0ebe01daee5 AS frontend

WORKDIR /src/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci

COPY frontend ./
RUN --mount=type=cache,target=/root/.npm npm run build

FROM rust:1.99-trixie@sha256:931c2c0531a8b143e044f0f3914dfe802f15bb3d1955f0b657e9359327ac2ffc AS chef

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

FROM debian:13-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f

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
