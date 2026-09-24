# Local measurements — 2026-09-24

All three sizes completed on local Wrangler/workerd. **The 50 and 100 MiB
graphs exceed Cloudflare's 128 MB production isolate memory ceiling.** The
emulator allowing them to complete is not evidence that they can deploy.

The workload uses seeded 4 KiB string properties, 32-node mutation blocks and
separate processes per size. MiB means 1,048,576 bytes of property payload;
indexes, node metadata, snapshots and retry results are additional storage.
Each latency run performs 10,000 reads after 100 warm-ups and 2,000 single-block
writes. Figures include local HTTP and Wrangler routing. The host is shared,
so these are exploratory measurements, not controlled production benchmarks.

| Payload | Nodes | Load (s) | DO restore (ms) | Client cold request (ms) | Read p50 / p99 (ms) | Write p50 / p99 (ms) |
|---|---:|---:|---:|---:|---:|---:|
| 10 MiB | 2,560 | 2.14 | 57 | 74.44 | 6.19 / 50.09 | 8.44 / 64.86 |
| 50 MiB | 12,800 | 28.19 | 273 | 297.49 | 6.00 / 47.45 | 9.69 / 65.25 |
| 100 MiB | 25,600 | 71.02 | 582 | 635.24 | 5.94 / 49.24 | 8.93 / 63.72 |

| Payload | Live Rust heap after restore (MiB) | Peak Rust heap (MiB) | WASM capacity (MiB) | SQLite database (MiB) | Snapshot parts |
|---|---:|---:|---:|---:|---:|
| 10 MiB | 23.01 | 27.28 | 36.38 | 10.44 | 9 |
| 50 MiB | 113.39 | 174.61 | 218.56 | 52.11 | 49 |
| 100 MiB | 226.77 | 348.76 | 415.94 | 104.25 | 98 |

Memory instrumentation is isolate-wide and excludes V8/JavaScript heap. Peaks
cover loading, automatic snapshots and recovery. Chunking protects the SQLite
row limit; it does not avoid the contiguous snapshot buffer or resident graph
indexes. The largest stored snapshot chunk was 1 MiB in every run. Even the
10 MiB result is only a lower bound on production isolate memory.

Cold measurement forces Durable Object eviction with `ctx.abort()`, then times
the first query and snapshot/WAL replay. It is an **object recovery**, not a fresh
WASM isolate startup: code compilation and isolate startup are not measured.
The header clock includes a 1 ms timer to let Workers' frozen JS clock advance.
The client duration is independently measured. Whole-process recovery is covered
by the SIGKILL/restart integration test.

## Validation

| Command | Result |
|---|---|
| `cargo check --locked --workspace --all-targets` | exit 0 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | exit 0, zero warnings |
| `cargo clippy --locked -p zega-cloud --lib --target wasm32-unknown-unknown -- -D warnings` | exit 0, zero warnings |
| `cargo test --locked --workspace` | exit 0; 917 passed, 0 failed, 2 ignored |
| `npm --prefix zega-cloud run build` | exit 0; release WASM |
| `npm --prefix zega-cloud test` | exit 0; 10 scenarios, 11 Node-reported tests including parent |
| `npm --prefix zega-cloud run corpus` | exit 0; 263 passed, 1 explicit offline skip; 263 exact native matches |
| `node zega-cloud/bench/local.mjs` | exit 0; all 12 phases completed |

The integration suite verifies graph isolation, native JSON, retained retry IDs,
12 concurrent retries, restart after acknowledged writes, injected failures after
WAL insertion and before result insertion, lost responses after commit, partial
documents, snapshot rollback/trim, automatic snapshots, and 10,000 queries after
eviction. The last case catches a wasm-bindgen proxy/finalizer lifetime bug that
short smoke tests missed.

**Negative proof:** temporarily bypassing `transactionSync`, rebuilding, and
running the integration tests makes the snapshot replacement fault scenario fail
(HTTP 500 instead of 503 because the snapshot/manifest no longer agree), exit 1.
The transaction wrapper was restored, rebuilt, and the complete local suite
then passed. The broken variant is not included.

A client missing a reply cannot know whether commit happened immediately before
the disconnect. Tests require absent *uncommitted* blocks and durable acknowledged
blocks; an unknown-outcome committed block is recovered through Request-Id.
Pre-commit faults roll back then abort the object. These local simulations do
not measure Cloudflare replication or remote power-loss durability.

## Evidence files

- [bench.jsonl](bench.jsonl): raw JSON from all 12 benchmark phases.
- [environment.json](environment.json): host, tool versions, base revision, WASM hash and command exits.
- [workspace-tests.log](workspace-tests.log): complete native test log and captured exit code.
- [local-tests.log](local-tests.log): final local integration output and exit code.
- [rollback-negative.log](rollback-negative.log): expected failure with transaction wrapper bypassed.
- [corpus.json](corpus.json): pinned corpus revision/digest, case verdicts and native parity.

The [main README](../README.md#ava-deploy-and-measure) contains exact commands for
Ava to deploy and measure from an authorized environment. No deployment, login,
Cloudflare credential use, remote latency or billing measurement was performed.

-codex
