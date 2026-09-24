# syntax=docker/dockerfile:1
# zega's native server for the Cloudflare Containers benchmark.
# Build context is the repository root (wrangler.jsonc sets image_build_context):
#   docker build -f zega-cloud/containers-bench/Dockerfile -t zega-bench .
FROM rust:1.96-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/.target \
    cargo build --locked --release -p zega-bench-server --target-dir /src/.target \
 && install -m 0755 /src/.target/release/zega-bench-server /zega-bench-server

# glibc's allocator (not musl's) so speed reflects a normal release build;
# distroless/cc carries only libc, libgcc and CA certificates.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /zega-bench-server /usr/local/bin/zega-bench-server
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/zega-bench-server", "--data", "/tmp/zega-data", "--host", "0.0.0.0", "--port", "8080"]
