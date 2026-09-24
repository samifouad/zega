# syntax=docker/dockerfile:1
# zega-bench-server (zega-cloud/containers-bench/server) for the Fly.io
# Machines benchmark. Same build as the Cloudflare image; only the runtime
# stage differs: the data directory is on the Fly volume mounted at /data, and
# the admin token comes from a Fly file secret written to /etc/zega/admin-token.
# Build context is the repository root:
#   docker build -f zega-cloud/fly-bench/server.Dockerfile -t zega-fly-server .
FROM rust:1.96-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/.target \
    cargo build --locked --release -p zega-bench-server --target-dir /src/.target \
 && install -m 0755 /src/.target/release/zega-bench-server /zega-bench-server

# Root, not distroless's nonroot: a Fly volume mounts root-owned, and the
# Machine is its own Firecracker VM.
FROM gcr.io/distroless/cc-debian12
COPY --from=build /zega-bench-server /usr/local/bin/zega-bench-server
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/zega-bench-server", "--data", "/data/zega", "--host", "0.0.0.0", "--port", "8080", "--token-file", "/etc/zega/admin-token"]
