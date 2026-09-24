# SPIKE: storage-backed zega on Cloudflare (not product code)

Two Workers, measured with one driver:

| config | what | option |
|---|---|---|
| `wrangler.toml` → `zega-storage-spike` | Worker → one Durable Object per graph. The Rust engine (WASM) keeps a 16 MB cache and reads nodes, adjacency and index entries synchronously from the object's SQLite. | **A** |
| `d1/wrangler.toml` → `zega-storage-spike-d1` | Worker → D1. Each query runs two ways: `engine` (one D1 call per storage call, what the engine seam would do) and `compiled` (the whole query as one SQL statement). | **B** |

R2 (option C) is not deployed; see the APS for why.

Both generate the same graph (node *i* has the same 5 fields and 3 `follows`
relationships in both). The DO target is covered by `../tests/parity.rs`: the
same executor returns exactly what `Zega::run_lang` returns.

## Run it (Ava; needs the Cloudflare account, Claude never does)

From this folder (`spikes/storage-backed/edge`), Node 18+, Rust 1.96 with the
`wasm32-unknown-unknown` target:

```sh
# 0. tools, once
cargo install worker-build --version 0.8.6 --locked   # or set WORKER_BUILD=/path/to/worker-build
npm install

# 1. option A: Durable Object (wrangler runs `node build.mjs` first)
npx wrangler deploy --config wrangler.toml

# 2. option B: D1
npx wrangler d1 create zega-storage-spike
#    put the printed database_id into d1/wrangler.toml, then:
npx wrangler deploy --config d1/wrangler.toml

# 3. benchmark (about 15-20 minutes; prints progress on stderr)
node bench/bench.mjs \
  --do https://zega-storage-spike.<subdomain>.workers.dev \
  --d1 https://zega-storage-spike-d1.<subdomain>.workers.dev \
  --out results/run-1.json

# 4. remove everything (the endpoints are open while deployed)
npx wrangler delete --config wrangler.toml
npx wrangler delete --config d1/wrangler.toml
npx wrangler d1 delete zega-storage-spike
```

Defaults: sizes 1k, 10k and 50k nodes, 100 requests per query shape
(10 for the full scan), 10 cold starts per shape. Change with
`--sizes 1000,10000,50000 --reps 100 --cold 10`. Run just one target by
leaving out `--do` or `--d1`. Hand back the `results/*.json` file.

**What it costs:** seeding writes about 13 rows per node (node, label,
2 index entries, 3 relationships, 6 adjacency rows) on each target:
about 0.8 M rows written per target for all three sizes, inside the
50 M rows/month included with Workers Paid. Reads are a few million
rows at most. Expected bill: well under $1.

## What it measures

Per target and size (`results/*.json`):

- `storage`: `database_bytes` (DO: `ctx.storage.sql.databaseSize`; D1: `meta.size_after`), row counts.
- `reads.<shape>` / `reads_engine` / `reads_compiled`: `client_ms` (end to end from the machine running the bench) and `server_ms` (what the front Worker saw: the Durable Object call including its hop, or the D1 calls), p50/p95/p99/mean.
- `per_request`: `billed_rows_read` and `billed_rows_written` (DO: the SQL cursors' own `rowsRead`/`rowsWritten`; D1: `meta.rows_read`/`rows_written`), SQL statements or D1 round trips, cache hits and misses.
- `writes.create` (one node, unique check, two index entries) and `writes.link` (find a node by its unique key, link it to three others).
- `cold` (DO only): the object is evicted (`ctx.abort()`), then the first request is timed; `confirmed_cold` counts responses that report the store was opened by that request. Isolate start-up is not forced, so this is an object cold start, not a fresh-isolate cold start.
- `heap_peak_bytes`: peak Rust heap in the isolate.

Shapes: `point` (by unique key), `one_hop` (3 nodes), `two_hop` (12 nodes),
`filter` (range index, ~20 rows), `scan_limit` (unindexed filter, limit 10,
~1,000 rows read), `scan_all` (unindexed, matches nothing: reads every node).

## Local run (no account)

```sh
npx wrangler dev --local --port 8813 --config wrangler.toml
npx wrangler dev --local --port 8814 --config d1/wrangler.toml
node bench/bench.mjs --do http://127.0.0.1:8813 --d1 http://127.0.0.1:8814 --sizes 1000 --reps 20 --cold 2
```
