# zega Cloud spike (APS 13, step 1)

A local-first Worker and one SQLite Durable Object per graph. Rust/`workers-rs`
0.8.6 calls `zega` directly on wasm32. The native workspace includes the crate;
Cloudflare dependencies and the allocator are wasm-only. The `zega` dependency
explicitly enables `durable-log`, which is **off by default** on every target. No account, credentials
or network deployment is needed to build or test it.

## Run locally

From the repository root (Node >=22, Rust and wasm32 target required):

```sh
umask 022
mkdir -p .tmp
chmod 700 .tmp
export CARGO_TARGET_DIR="$PWD/.target" TMPDIR="$PWD/.tmp"
rustup target add wasm32-unknown-unknown
cargo install worker-build --version 0.8.6 --locked --root "$PWD/.tmp/tools"
npm --prefix zega-cloud ci
npm --prefix zega-cloud run build
cargo build --locked -p zega-cloud --bin cloud-native
npm --prefix zega-cloud test
npm --prefix zega-cloud run corpus
node zega-cloud/bench/local.mjs
```

The tests spawn **local-only** `wrangler dev`, isolate its config/auth directory,
remove inherited Cloudflare credential variables, and persist SQLite under
`.tmp/`. They do not deploy or log in. `build.mjs` uses the local `worker-build`
above, otherwise the executable on PATH; an explicit executable is also accepted.

For an interactive endpoint:

```sh
cd zega-cloud
npm run dev -- --port 8787 --var SPIKE_CONTROLS:true
```

Requests use the native server's JSON body and envelope:

```sh
curl http://127.0.0.1:8787/g/example/zql \
  -H 'Content-Type: application/json' -H 'Request-Id: example-1' \
  --data '{"schema":"schema { type Person { name: String } }","query":"mutation { Person(name: \"Ada\") { name } }"}'
curl http://127.0.0.1:8787/g/example/zql \
  -H 'Content-Type: application/json' \
  --data '{"schema":"schema { type Person { name: String } }","query":"query { Person { name } }"}'
```

`{schema, query, document?, sources?}` matches `POST /zql` on native. With
`document: true`, `query` is a complete `.zql` document. `text/plain` also accepts
a complete document. Imports require a `sources` map of location to raw UTF-8,
just like the browser host; no filesystem or HTTP import transport runs inside
the object. Schema travels with each request, as it does on the native server.

`Request-Id` is required for mutations (1–128 bytes). A retained ID with a different
request body returns 409. Successful retries return the original result and
perform only SELECTs. The most recent **4096 request IDs per graph** are retained;
older IDs may be applied again. Requests and graph names are scoped by the
Durable Object derived with `idFromName(graph)`.

## Storage and transaction boundaries

* `wal(seq INTEGER PRIMARY KEY, entry BLOB)`: exactly one native WAL entry per
  journalled mutation block: little-endian u64 payload length, little-endian u32
  CRC32, bincode payload. The `ZWAL` file header is a file header, not an entry,
  and is not repeated in the table. There is no graph-to-SQL mapping.
* `snapshot(part INTEGER PRIMARY KEY, bytes BLOB)`: the existing native bincode
  snapshot, divided into 1 MiB chunks. Serialization borrows the graph maps to
  avoid making another complete graph copy; the encoded bytes are unchanged.
* `meta(key TEXT PRIMARY KEY, value BLOB)`: snapshot covered sequence, byte/chunk
  counts, SHA-256 and next node/relationship IDs. The ID metadata prevents reuse
  after deleting the highest ID, which the historical snapshot format alone
  cannot represent.
* `requests(id, block, fingerprint, result)`: block results and completed response
  bodies. Successful earlier blocks remain cached if a later block is interrupted.

The target-neutral `AppendTarget` seam is public with `zega/durable-log`. Only
`zega-cloud` implements SQLite storage; the engine has no Cloudflare or
`wasm-bindgen-futures` dependency. Regular WASM retains its empty in-memory WAL. A small
`write_entry` extension lets row targets accept a complete frame while preserving
native file targets' existing streaming write behavior. SQLite append is INSERT;
rollback is `DELETE WHERE seq > last_good`. Replay reads ordered rows and uses the
same bounded bincode decoder as native, after checking frame length/CRC. A gap or
corrupt committed SQL row fails closed. Native torn-tail repair stays native.

`ZqlProgram` calls the same parser and executor as `run_lang`/`apply_zql`, and lets the
host wrap **each block**, including its retry result, in `transactionSync`. A Rust
error must throw *inside* that callback to roll back SQLite. Engine journal errors
restore graph state; failures after engine success discard the engine and reload
committed storage before another request can use it. No full-graph copy is taken
for a mutation. Blocks await `storage.sync()` separately, so a later failure does
not undo earlier committed blocks. A failing document identifies its block in
`X-Zega-Failed-Block` (one-based). JSON diagnostics remain native-compatible.

Replies use the Durable Object output gate; `allowUnconfirmed` is never used.
A final response cache is written before returning. Snapshot replacement, metadata
and WAL trim share one synchronous storage transaction, automatically triggered
at 8 MiB of WAL bytes (`SNAPSHOT_BYTES` in `wrangler.toml`). Entries after the
snapshot's covered sequence are replayed on a cold start. Retry records survive
both snapshots and trimming.

There are two small `workers-rs` gaps in this spike:

1. `transactionSync` and `storage.sync` need direct wasm-bindgen bindings.
2. `ctx.abort()` is deliberately uncatchable. Calling it while the Rust executor
   polls a future stranded the shared executor in local workerd, even with a
   `catch` binding. `entry.mjs` is a lifecycle wrapper: it waits for Rust to finish,
   releases its graph/JS reference, then calls `ctx.abort()` in JavaScript. Routing, ZQL,
   the actual Durable Object implementation and storage remain Rust. There is no
   JS engine or JS WAL codec, and `zega-wasm` does not enable `durable-log`. The wrapper leaves
   instance destruction to wasm-bindgen's finalizer: calling `free()` through
   the SDK's proxy leaves the underlying finalizer token registered, causing a
   second free on a later GC (caught by the 10,000-query run after eviction).

## Cold start, instrumentation and limits

The first request constructs the in-memory engine, restores the snapshot and
replays the log. It logs a JSON cold-start event and supplies:

* `X-Zega-Cold-Start-Ms`, on that request only. Workers freeze JS clocks during
  CPU execution, so the measurement waits a 1 ms timer after restore; it includes
  timer scheduling and is a coarse wall measurement. `bench/cold.mjs` also reports
  the client's complete HTTP duration. This forces object recovery within the
  running isolate; it does not include fresh-isolate compilation/startup.
* `X-Zega-Heap-Bytes` / `X-Zega-Peak-Heap-Bytes`: current and lifetime peak requested
  Rust allocation bytes, measured by the allocator.
* `X-Zega-Wasm-Bytes`: linear-memory capacity. All three memory figures are
  **isolate-wide**, include other resident objects, and exclude V8/JS heap.
  WASM capacity does not shrink on object eviction. The local bench driver starts
  a separate process for each size so sizes do not contaminate each other's heaps.
* `X-Zega-Wal-Seq`, and `X-Zega-Retry: true` when an earlier result is reused.

Bodies are limited to 1 MiB; WAL entries and cached results to 1900 KiB, leaving
room below SQLite's 2 MB row limit. An oversized entry/result rolls the block back.
Snapshots are chunked, but encoding/restoring still allocates one contiguous
snapshot buffer; this is a spike, not a paging implementation. The measured Rust
heap is a lower bound on total isolate memory. A successful local run above
128 MiB does **not** demonstrate that it fits Cloudflare's production limit.

`SPIKE_CONTROLS` is false in the checked-in config. Enabling it exposes `_spike-stats`,
`_spike-graph`, `_spike-check`, `_spike-snapshot`, `_spike-snapshot-fail` and
`_spike-evict` under `/g/<graph>/`, plus deterministic write-failure headers. These
are unauthenticated measurement/test controls; the deployment is a temporary
spike, not a multi-tenant authenticated service. The query endpoint also has no
application authentication in this step. No routes or custom domains are configured.

## Regular WASM isolation

`durable-log = []` is an explicit, default-off engine feature. It exposes the
host journal, framed replay, checkpoint metadata and per-block ZQL program.
Only WASM with that feature owns an append target and serializes WAL entries.
File storage remains native-only. The borrowed snapshot optimization is also
feature-gated; ordinary browser parsing, snapshot allocation and WAL behavior
stay as on `main`. These APIs use Rust types and synchronous `std::io`; no
Cloudflare or async-JavaScript dependency enters the engine.

From a clean checkout:

```sh
node browser/scripts/check-wasm.mjs
```

This checks native and wasm32 dependency trees, rejects `durable-log` feature
unification, rebuilds with the repo's `rebuild-wasm.mjs`, and compares SHA-256
against the **pre-build committed** `browser/pkg` hash. The dedicated CI job
pins Rust 1.96.0 and wasm-pack 0.15.0, requires sccache, and needs no Cloudflare
credentials. `rebuild-wasm.mjs` now remaps checkout/Cargo/sysroot source paths to
`/zega`, `/cargo`, `/rust`; the prior artifact contained iMac worktree paths and
could not be reproduced elsewhere. JS glue and declarations are unchanged.

Both `origin/main` and the feature-gated branch were built at the same path with
this script, toolchain and flags: **2,745,640 bytes each**. Every section except
the data section is byte-identical, including all compiled code, imports and
exports. The only nine different bytes encode eight moved Rust panic-location
line numbers. [The comparison report](evidence/wasm-comparison.json) enumerates
every byte offset, source line and old/new value. The following verifier rejects
any other change or size growth:

```sh
python3 zega-cloud/test/compare-wasm.py MAIN.wasm BRANCH.wasm MAIN_REV
```

The ungated spike fails the artifact guard; injecting `durable-log` into
`zega-wasm` fails the feature guard, and injecting a wasm32-only `worker`
dependency fails the target dependency-tree guard. No compiled browser code has been added.
`zega-wasm` remains the existing wasm-bindgen JavaScript-host target; this does
not introduce a new host-neutral ABI for non-JS runtimes. The engine seam remains
available for such a separate target without Cloudflare assumptions (APS 13).

## Verification

`npm test` exercises real local workerd/SQLite: mutation/query envelopes, graph
isolation, forced DO eviction, whole-process SIGKILL/restart, retries after restart
and trim, concurrent retries, refusal part-way through a block, faults after WAL
INSERT and before result INSERT, a lost response after commit, chunked snapshots,
a failed snapshot replacement, and automatic snapshot thresholds.

A killed request can have committed just before its reply was lost. No server can
deduce receipt by the client at that boundary. The suite distinguishes **uncommitted
blocks**, which must not appear, from **unknown-outcome committed blocks**, which
must return the saved result on retry. Every acknowledged block must survive.
The injected pre-commit failures test rollback before abort; this is not a remote
power-loss or replicated-durability test.

`test/corpus.mjs` pins zegadb/testsuite at
`d9684641b3a77f772f920c47bb40cc7d5b0e2995`, reuses its loader/grader, and compares
complete native/Worker outcomes including exact JSON number spellings and
complete diagnostics. Parse cases use the gated parser endpoint; executable
cases run `/g/<fresh-graph>/zql`. All local raw import fixtures are supplied to
both hosts. The corpus's opt-in remote import is explicitly skipped offline.
The native oracle calls the same engine revision as the Worker, and every case
also checks the corpus's independent expectations.

```sh
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo clippy --locked -p zega-cloud --lib --target wasm32-unknown-unknown -- -D warnings
cargo test --locked --workspace > .tmp/workspace-tests.log 2>&1
code=$?
cat .tmp/workspace-tests.log
printf 'cargo test exit=%s\n' "$code"
```

[Local evidence and measurements](evidence/README.md) include all three sizes,
command exits and the negative rollback test. The 50 and 100 MiB payloads exceed
the production memory ceiling. Remote CPU, V8 heap,
billing, replication behavior and real Cloudflare latency remain for Ava to measure.

## Ava: deploy and measure

From an already authorized Cloudflare environment, after reviewing the draft:

```sh
# First run the local build commands above. Only Ava runs this subshell.
(
  set -e
  cd zega-cloud
  npm exec -- wrangler deploy --var SPIKE_CONTROLS:true
  # Supply the workers.dev URL printed by Wrangler.
  endpoint='https://zega-cloud-spike.<your-workers-subdomain>.workers.dev'
  run=$(date -u +%Y%m%dT%H%M%SZ)
  for size in 10 50 100; do
    graph="bench-${run}-${size}"
    node bench/load.mjs "$endpoint" "$graph" "$size"
    node bench/cold.mjs "$endpoint" "$graph"
    node bench/read.mjs "$endpoint" "$graph"
    node bench/write.mjs "$endpoint" "$graph"
  done
)
```

Each script prints JSON. Loads use seeded 4 KiB strings and 32-node JSON mutation
blocks, targeting ~10/50/100 **MiB of property payload**, with actual SQLite and
snapshot sizes also reported. Use fresh graph names for independent runs; load
Request-Ids are deterministic so a retry resumes the same load. Reads measure
10,000 sequential point queries after 100 warm-ups; writes measure 2,000 single
mutation blocks, each with a fresh request ID. The optional final argument to
read/write changes the sample count. Quantiles use nearest rank. HTTP timings
include routing, JSON, transport and durable acknowledgment. If a load fails,
record it as a size limit; do not report latency on its smaller surviving prefix
as the requested size. The local driver does this automatically.

- [APS 13](https://github.com/zegadb/aps/issues/13)
- [APS 10](https://github.com/zegadb/aps/issues/10)
- [Cloudflare SQLite API](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
- [Cloudflare abort semantics](https://developers.cloudflare.com/durable-objects/api/state/#abort)
- [workers-rs](https://github.com/cloudflare/workers-rs)

-codex
