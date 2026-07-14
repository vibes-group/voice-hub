FROM node:24.15-bookworm-slim AS frontend

WORKDIR /src/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci

COPY frontend ./
RUN --mount=type=cache,target=/root/.npm npm run build

FROM golang:1.26-bookworm AS build

WORKDIR /src

COPY backend/go.mod backend/go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    go mod download

COPY backend ./

RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 GOOS=linux \
    go build -ldflags="-s -w" -o /out/voice-hub-app ./cmd/server

FROM debian:12-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates wget \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd -r app && useradd -r -g app -u 10001 -d /app -s /sbin/nologin app

WORKDIR /app

COPY --from=build /out/voice-hub-app /app/voice-hub-app
COPY --from=frontend /src/frontend/dist /app/web

# Persistent state (connection-password hash). Mounted as a volume in compose.
RUN mkdir -p /app/data && chown app:app /app/data

ENV APP_ADDR=:8080
ENV APP_WEB_DIR=/app/web
ENV APP_UPDATES_DIR=/app/updates

EXPOSE 8080

USER app:app

CMD ["/app/voice-hub-app"]
