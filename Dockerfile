FROM node:22-alpine AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

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
ENV NEWKITINE_WEB_ROOT=/app/web
ENV NEWKITINE_CONFIG=/config/newkitine.toml
USER newkitine
EXPOSE 8080
ENTRYPOINT ["/app/newkitine"]
