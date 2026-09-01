# syntax=docker/dockerfile:1.7

FROM node:22-bookworm-slim AS frontend

WORKDIR /work/frontend

COPY frontend/package*.json ./
RUN npm install

COPY frontend ./
RUN npm run build

FROM rust:1.98-bookworm AS build

WORKDIR /work

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=frontend /work/frontend/dist /work/frontend/dist

RUN cargo build --release --locked --bin telegram-s3

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --home-dir /var/lib/telegram-s3 --shell /usr/sbin/nologin --uid 10001 telegram-s3 \
    && install -d -m 0700 -o telegram-s3 -g telegram-s3 /var/lib/telegram-s3/data /var/lib/telegram-s3/session /var/lib/telegram-s3/metadata \
    && install -d -m 0755 -o telegram-s3 -g telegram-s3 /var/lib/telegram-s3/ui

COPY --from=build /work/target/release/telegram-s3 /usr/local/bin/telegram-s3
COPY --from=frontend --chown=telegram-s3:telegram-s3 /work/frontend/dist/. /var/lib/telegram-s3/ui/
COPY docker/entrypoint.sh /usr/local/bin/telegram-s3-entrypoint

RUN chmod 0755 /usr/local/bin/telegram-s3 /usr/local/bin/telegram-s3-entrypoint \
    && chown -R telegram-s3:telegram-s3 /var/lib/telegram-s3

ENV TELEGRAM_ADMIN_UI_DIST_DIR=/var/lib/telegram-s3/ui

USER telegram-s3
WORKDIR /var/lib/telegram-s3

EXPOSE 9000

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD curl -fsS http://127.0.0.1:9001/healthz || exit 1

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/telegram-s3-entrypoint"]
CMD ["server"]
