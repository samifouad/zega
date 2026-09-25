# syntax=docker/dockerfile:1
#
# Zega Cloud's Pro graph image (registry.fly.io/zega-g:latest): one Fly
# Machine per graph, running `zega start` (crate zega-cli, [[bin]] name =
# "zega") as an HTTP server. Data lives on the Fly volume the control plane
# mounts at /data; the per-graph bearer token is the file Fly's `files`
# (raw_value) writes at /etc/zega/token before the machine ever starts.
# Source of truth for both: zegadb/cloud src/machines/fly.ts (FlyMachines),
# whose `config()` sets:
#   init.cmd: ["start", "--host", "::", "--port", "8080",
#              "--data", "/data/zega", "--token-file", "/etc/zega/token"]
#   services: [{ internal_port: 8080, ... }]
# Fly's `init.cmd` replaces CMD, not ENTRYPOINT, so ENTRYPOINT here (via
# entrypoint.sh) must accept that exact argv and end up running the `zega`
# binary with it unchanged — mirrors zega-cloud/fly-bench/server.Dockerfile's
# shape, with the same crate substitution documented in
# zegadb/cloud scripts/setup-fly-test.sh's header.
#
# Build context is the repository root:
#   docker build --platform linux/amd64 -f docker/zega-g/Dockerfile \
#     -t registry.fly.io/zega-g:latest .
# See docs/cloud-image.md for the full build/push/verify runbook.

FROM rust:1.96.0-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/.target \
    cargo build --locked --release -p zega-cli --target-dir /src/.target \
 && install -m 0755 /src/.target/release/zega /zega

# debian-slim, not distroless: a fresh Fly volume mounts at /data owned
# root:root 0755 (community-documented:
# https://community.fly.io/t/fly-volumes-getting-permission-denied/1773), so
# entrypoint.sh needs a shell and `chown` to hand it to the non-root `zega`
# user before the server ever runs as that user — distroless has neither.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --no-create-home --shell /usr/sbin/nologin --uid 10001 --user-group zega \
 && mkdir -p /data /etc/zega \
 && chown zega:zega /data \
 # A placeholder, not real content: Fly's `files` (raw_value) and a plain
 # `docker run -v host/token:/etc/zega/token` both write/mount onto an
 # existing regular file reliably; a target path with no file there yet has
 # been observed to come up as a directory instead (Docker Desktop bind
 # mounts, verified while writing this image), which then fails
 # `zega start --token-file` with "Is a directory".
 && touch /etc/zega/token

COPY --from=build /zega /usr/local/bin/zega
COPY docker/zega-g/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod 0755 /usr/local/bin/entrypoint.sh

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
