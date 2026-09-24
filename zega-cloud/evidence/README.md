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
| 10 MiB | 2,560 | 2.21 | 61 | 78.77 | 5.87 / 46.63 | 8.67 / 61.77 |
| 50 MiB | 12,800 | 20.61 | 288 | 311.22 | 6.17 / 47.95 | 9.33 / 63.94 |
| 100 MiB | 25,600 | 63.97 | 577 | 629.71 | 6.34 / 50.90 | 9.26 / 63.99 |

| Payload | Live Rust heap after restore (MiB) | Peak Rust heap (MiB) | WASM capacity (MiB) | SQLite database (MiB) | Snapshot parts |
|---|---:|---:|---:|---:|---:|
| 10 MiB | 23.01 | 27.28 | 36.38 | 10.44 | 9 |
| 50 MiB | 113.39 | 174.61 | 218.56 | 52.11 | 49 |
| 100 MiB | 226.77 | 348.76 | 415.94 | 104.25 | 98 |

The first 50 MiB load lost its local connection after 47.25 MiB had been acknowledged (HTTP 500, Network connection lost). The driver skipped cold/read/write for that incomplete load and returned exit 1 while completing 10 and 100 MiB. A fresh-process `node zega-cloud/bench/local.mjs 50` rerun completed all four phases, exit 0. The table uses that complete rerun; the original failure and worker log are retained. Its cause is unconfirmed and is not classified as a memory limit.

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

## Regular WASM comparison

Baseline: `origin/main` at `7fa1b47c836cd577d1371fdc84881ac6d7825881`; branch at the
commit recorded in [wasm-comparison.json](wasm-comparison.json). Both trees were
exported with `git archive` into the **same directory** and built back to back on the
iMac with Rust 1.96.0 and wasm-pack 0.15.0 (`wasm-pack build zega-wasm --target web
--release --locked`, `--remap-path-prefix` for checkout, Cargo home and sysroot, one
fresh `CARGO_TARGET_DIR`), in the order main, branch, main.

| Build | WASM bytes | SHA-256 |
|---|---:|---|
| main | 2,757,992 | `aab5f8b42960598a8dc4fcb2fae29988355828a613241de515415ac029047eb3` |
| branch | 2,757,992 | `e4fefa5b07a5a8e2d747a50fca7ec85a0ca69a3e20058acb64a3d57f7bc5d02a` |
| main again | 2,757,992 | `aab5f8b42960598a8dc4fcb2fae29988355828a613241de515415ac029047eb3` |
| main source, other directory | 2,743,134 | `ebcf6644ae8261f3e76c0161bc991311cc3d27ae982970e4b5f17f0680ea20e6` |

**Zero size growth.** The `code` section is byte-identical (all 2,170 function bodies),
as are type, import, function, table, memory, global, export, element, datacount and
custom sections. Exactly nine bytes differ, all in `data`: seven panic locations in
`lib.rs` move down five source lines and one in `wal/mod.rs` moves from line 231 to
261. `test/compare-wasm.py` checks that no other byte differs and that each old/new
line holds identical source. The last row shows why a committed hash is not a gate:
the same source built in another directory is a different binary.

The CI gate (`node browser/scripts/check-wasm.mjs`) reads Cargo's resolved graph only:
native and wasm32 dependency trees free of Cloudflare crates, and no `durable-log`
through feature unification. Negative checks each exited 1 as expected: a temporarily
enabled `zega/durable-log` in `zega-wasm`, and a temporarily injected wasm32-only
`worker` crate. Both injections were reverted. `browser/pkg` is unchanged from `main`.

## Validation

| Command | Result |
|---|---|
| `cargo check --locked --workspace --all-targets` | exit 0 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | exit 0, zero warnings |
| `cargo clippy --locked -p zega-cloud --lib --target wasm32-unknown-unknown -- -D warnings` | exit 0, zero warnings |
| `cargo test --locked --workspace` | exit 0; 930 passed, 0 failed, 2 ignored |
| `npm --prefix zega-cloud run build` | exit 0; release WASM |
| `npm --prefix zega-cloud test` | exit 0; 10 scenarios, 11 Node-reported tests including parent |
| `npm --prefix zega-cloud run corpus` | exit 0; 263 passed, 1 explicit offline skip; 263 exact native matches |
| `node zega-cloud/bench/local.mjs` and `... 50` | initial invocation exit 1 (50 MiB connection loss); fresh 50 MiB rerun exit 0; all 12 requested phases measured |
| `npm --prefix browser test` | exit 0; 35 passed (run at `036e9a7` against a rebuilt `browser/pkg` since reverted to `main`; `browser/` is now unchanged) |
| `node browser/scripts/check-wasm.mjs` | exit 0; both dependency trees and the feature check |
| `python3 zega-cloud/test/compare-wasm.py MAIN.wasm BRANCH.wasm MAIN_REV` | exit 0; all nine byte differences explained; zero size growth |

The integration suite verifies graph isolation, native JSON, retained retry IDs,
12 concurrent retries, restart after acknowledged writes, injected failures after
WAL insertion and before result insertion, lost responses after commit, partial
documents, snapshot rollback/trim, automatic snapshots, and 10,000 queries after
eviction. The last case catches a wasm-bindgen proxy/finalizer lifetime bug that
short smoke tests missed.

**Initial spike negative proof** (retained from `75fae50`): temporarily bypassing
`transactionSync`, rebuilding, and
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

- [wasm-comparison.json](wasm-comparison.json): sizes, hashes, identical sections and every changed panic-location byte.
- [wasm-ci-guard.log](wasm-ci-guard.log): successful local run of the CI gate.
- [wasm-dependencies.log](wasm-dependencies.log): native/wasm32 trees and disabled durable-log verification.
- [wasm-negative-feature.log](wasm-negative-feature.log), [wasm-negative-cloudflare.log](wasm-negative-cloudflare.log): expected guard failures.
- [wasm-main-build.log](wasm-main-build.log), [wasm-branch-build.log](wasm-branch-build.log): same-directory rebuild logs for main and branch.
- [browser-tests.log](browser-tests.log): 35 passing browser tests (earlier run; `browser/` is unchanged from `main`).
- [workspace-check.log](workspace-check.log), [workspace-clippy.log](workspace-clippy.log), [cloud-clippy.log](cloud-clippy.log): passing native and feature-on wasm32 gates.
- [bench.jsonl](bench.jsonl): 12 successful phases plus the retained failed load.
- [bench-attempt-1.log](bench-attempt-1.log), [bench-retry-50.log](bench-retry-50.log), [bench-50-first-worker.log](bench-50-first-worker.log): complete attempts and the unexplained local connection loss.
- [wasm-dependency-trees.log](wasm-dependency-trees.log): full native and wasm32 normal dependency trees.
- [environment.json](environment.json): host, tool versions, base revision, WASM hash and command exits.
- [workspace-tests.log](workspace-tests.log): complete native test log and captured exit code.
- [local-tests.log](local-tests.log): final local integration output and exit code.
- [rollback-negative.log](rollback-negative.log): expected failure with transaction wrapper bypassed.
- [corpus.json](corpus.json): pinned corpus revision/digest, case verdicts and native parity.

The [main README](../README.md#ava-deploy-and-measure) contains exact commands for
Ava to deploy and measure from an authorized environment. No deployment, login,
Cloudflare credential use, remote latency or billing measurement was performed.

-codex
