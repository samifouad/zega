# syntax=docker/dockerfile:1
# The bench client that runs inside Fly (same region as the servers), so the
# numbers carry no Calgary network. Node only; no npm dependencies.
# Build context is the repository root:
#   docker build -f zega-cloud/fly-bench/client.Dockerfile -t zega-fly-client .
FROM node:22.20.0-bookworm-slim
WORKDIR /bench
# bench.js is ESM; its package.json says so.
COPY zega-cloud/containers-bench/package.json containers-bench/package.json
COPY zega-cloud/containers-bench/src/bench.js containers-bench/src/bench.js
COPY zega-cloud/fly-bench/common.mjs zega-cloud/fly-bench/client.mjs fly-bench/
USER node
ENTRYPOINT ["node", "/bench/fly-bench/client.mjs"]
