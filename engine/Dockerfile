# syntax=docker/dockerfile:1

# EDITION=core runs cadri-engine (MIT); EDITION=ee runs cadri-engine-ee (commercial, see ee/LICENSE).
ARG EDITION=core

FROM rust:1-bookworm AS build
ARG EDITION
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    set -eu; \
    case "$EDITION" in \
      core) bin=cadri-engine ;; \
      ee) bin=cadri-engine-ee ;; \
      *) echo "EDITION must be 'core' or 'ee', got '$EDITION'" >&2; exit 1 ;; \
    esac; \
    cargo build --release --locked --bin "$bin"; \
    install -D "target/release/$bin" /out/cadri-engine

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 cadri \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin cadri \
    && install -d -o 10001 -g 10001 /data
COPY --from=build /out/cadri-engine /usr/local/bin/cadri-engine
ENV CADRI_BIND=0.0.0.0:8080 \
    CADRI_DB=/data/transcripts.db
VOLUME /data
EXPOSE 8080
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/cadri-engine"]
