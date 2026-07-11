FROM node:22-alpine AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM debian:bookworm-slim AS geoip
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL "https://download.db-ip.com/free/dbip-country-lite-$(date -u +%Y-%m).mmdb.gz" \
    | gunzip > /dbip-country-lite.mmdb

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release --bin newkitine

FROM debian:bookworm-slim
RUN useradd -m -u 1000 newkitine
WORKDIR /app
COPY --from=build /src/target/release/newkitine /app/newkitine
COPY --from=web /web/dist /app/web
COPY --from=geoip /dbip-country-lite.mmdb /app/dbip-country-lite.mmdb
ENV NEWKITINE_WEB_ROOT=/app/web
ENV NEWKITINE_WEB_BIND=0.0.0.0:8080
ENV NEWKITINE_CONFIG=/config/newkitine.toml
ENV NEWKITINE_GEOIP_DB=/app/dbip-country-lite.mmdb
USER newkitine
EXPOSE 8080
ENTRYPOINT ["/app/newkitine"]
