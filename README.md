# zega

An embeddable graph database. Written in Rust.

<p align="left">
  <img src="image.png" alt="zega browser" width="70%">
</p>

zega provides a property graph with a Cypher-inspired query language in
one engine, one binary, one dependency. It runs in-process in
any Rust application, persists through a write-ahead log with snapshots, and
compiles to WebAssembly for the browser.

No separate database server to install unless you want one. The server is a
single binary that speaks HTTP/JSON. But you can absolutely run it in the browser, 
just like [cqx](https://cqx.bio) does for running queries without a server:

<p align="left">
  <img src="cqx-zega-deka.png" alt="zega in the browser!" width="70%">
</p>

## Features

- **Property graph** — labelled nodes, typed relationships, property indexes,
  and variable-length traversals (`-[*1..3]->`).
- **Path finding** — `road *path -> Junction(name: "B")` returns one route
  with its nodes, edges and cost: fewest edges, `by &km` (Dijkstra), or
  `by &km toward at` (A*, with `km: Float<km>`). See [paths](docs/path.md).
- **ZQL** — a Cypher-inspired query language: `MATCH`, `CREATE`, `MERGE`,
  `SET`, `DELETE`/`DETACH DELETE`, `WHERE`, `WITH`, `UNWIND`, `FOREACH`,
  `ORDER BY`/`SKIP`/`LIMIT`, aggregates (`count`, `sum`, `avg`, `min`, `max`,
  `collect`), `CASE`, and scalar functions.
- **Embeddable** — `zega` is a library first. Open a database with two
  lines of Rust and run graph queries against it.
- **WebAssembly** — `zega-wasm` exposes the same engine to JavaScript,
  in-memory, in the browser.
- **Durable** — CRC32-framed write-ahead log with group commit, torn-write
  detection, snapshots, and automatic WAL replay on open.
- **Optional server** — a tokio/axum HTTP server with token auth and
  ZQL execution on the blocking pool.
- **JWT auth + policy engine** — HS256/RS256 token verification and row-level
  access control when you need multi-tenant semantics.

## Use the CLI

Build the native executable with `cargo build --locked --release -p zega-cli`.
The resulting binary is `.target/release/zega` when using the repository's
`CARGO_TARGET_DIR=.target` convention (`.exe` on Windows).

```sh
zega --help
zega --version
zega start --data ./data
# http://127.0.0.1:9342
zega explorer --data ./data
# http://127.0.0.1:9343
```

The CLI locks its data directory for the life of the process; starting another
CLI process against that directory fails instead of sharing the WAL. `start` defaults to port **9342** (ZEGA on a
phone keypad); `explorer` defaults to **9343**. Both accept `--port 0` to ask the
OS for an available port and print the actual URL. The default data directory
is `./zega-data`. The explorer serves the embedded browser bundle and uses the
native database for every query, import and graph edit; it never opens a browser
automatically or reseeds an existing database. Its editor currently loads Monaco
from the same CDN used by the standalone explorer, so editor startup needs a
network connection even though the application assets and wasm are embedded.

`zega start --host 0.0.0.0 --token-file ./token --data ./data` enables an
explicit remote bind. The file must contain one nonempty bearer token (a final
newline is fine). With a token file, every database route requires
`Authorization: Bearer <token>`, including `/health`. Without one, only the exact
address `127.0.0.1` is accepted. Explorer always binds to `127.0.0.1`.
Configuration is through flags; product behavior does not use environment
variables. The old server executable, environment-variable startup and `/cql`
route have been removed.

### Run ZQL over HTTP

`POST /zql` accepts `{ "schema": "...", "query": "..." }` and returns
`{ "ok": true, "result": ... }`. Errors return `{ "ok": false, "error": "..." }`
with an error status. `GET /health` returns `{ "ok": true }`.

```sh
curl -s http://127.0.0.1:9342/zql \
  -H 'Content-Type: application/json' \
  -d '{"schema":"type Player { name: String }","query":"mutation json [\"./players.json\"] { Player(name: $Name) { name } }"}'
curl -s http://127.0.0.1:9342/zql \
  -H 'Content-Type: application/json' \
  -d '{"schema":"type Player { name: String }","query":"{ Player { name } }"}'
```

Native load paths resolve against the process cwd. HTTP(S) loads use the library
loader. `--allow-private-imports` explicitly permits private/loopback URLs for
trusted callers; the default rejects them. Browser file-picker imports supply an
optional `sources` object mapping literal ZQL locations to raw text. The engine
parses and inserts that text. To execute a full ZQL file, set `document: true`
and put the document in `query` (no separate schema needed).

The explorer also uses authenticated `/graph` read/clear and graph edit routes.
Requests execute on the blocking pool under a shared database gate; slow native
loads do not block the HTTP health worker. See [data loading](docs/data-loading.md)
for format, limits and WAL semantics.

Release manifests contain `zega-darwin-arm64`, `zega-darwin-x64`, `zega-linux-x64`
and `zega-windows-x64.exe`, with SHA-256 hashes. These are the artifact keys
expected by the site's installer.

## Use it as a library

Add `zega` to your `Cargo.toml` (path or git dependency for now):

```rust
use zega::Zega;

// Disk databases replay their WAL on open; in_memory() has the same ZQL API.
let db = Zega::open("./data").build()?;
let schema = "type Person { name: String }";
db.run_lang(schema, r#"mutation { Person(name: "Ada") { name } }"#)?;
let result = db.run_lang(schema, "{ Person { name } }")?;
assert_eq!(result[0]["name"], "Ada");

```

Durability is configurable on the builder:

```rust
let zega = Zega::open("./data")
    .wal_flush_every_write()   // fsync every append
    .wal_flush_interval(10)    // or group-commit every 10 ms
    .traversal_work_budget(1_000_000)
    .build()?;
```

## Embed the engine from npm

The `zegadb` package is prepared for browser bundlers and Node:

```js
import { createDatabase } from 'zegadb';

const db = await createDatabase();
try {
  console.log(JSON.parse(db.run(
    'type Person { name: String }',
    'mutation { Person(name: "Ada") { name } }',
  )));
} finally {
  db.free();
}
```

See the [package API and bundler notes](npm/README.md) and
[build, consumer checks, and disabled release setup](npm/PUBLISHING.md).
Registry publication is pending; local consumers install `artifacts/zegadb-0.1.0.tgz`.
The repository's own explorer stays in `browser/` and uses the WASM crate directly.

## Build the raw browser bindings

`zega-wasm` wraps an in-memory database for JavaScript via wasm-bindgen:

```bash
cd zega-wasm
wasm-pack build --target web
```

```javascript
import init, { ZegaWasm } from "./pkg/zega_wasm.js";

await init();
const db = new ZegaWasm();

const schema = 'type Person { name: String }';
db.run(schema, 'mutation { Person(name: "Ada") { name } }');
console.log(JSON.parse(db.run(schema, '{ Person { name } }')));

```

## Formatting ZQL and JSON

`zega fmt paths…` formats files or directories recursively (`*.zql` and `*.json`).
`zega fmt --check paths…` lists every file that would change and exits 1;
`zega fmt --stdin` reads source from stdin and writes the canonical layout.
Invalid or incomplete input is left unchanged. There are no style options.

The explorer uses the same formatter through WASM. Press ⌘S / Ctrl-S or
**Format** to format and save the active schema or query pane. Formatting is
undoable and preserves the cursor line (clamped when lines are removed).

The locked layout in [APS 12](https://github.com/zegadb/aps/issues/12) keeps
1–2 plain selection fields inline, opens schema types with 2+ fields, separates
types and top-level blocks with one blank line, and uses only `//` comments.
Display views keep 1–2 types inline and open 3+ types one per line. Per-type
attributes follow the same threshold inside parentheses. `String<url>` keeps its
angle brackets tight. Every `then` and `display { skip }` opens as a top-level
block; discovery sub-blocks stay compact when they fit, and boolean chains over
80 columns put each operand on its own line.
JSON objects with 1–2 members and scalar arrays stay inline when they fit 80
columns. JSON key order, number spelling and string escapes are preserved;
invalid input is returned unchanged. Directories include both `.zql` and `.json`.
Use `zega fmt --stdin --lang json` for JSON on standard input. The explorer uses
the same WASM formatter for its JSON import preview and result views.

## The query language

ZQL has explicit schemas, mutations and graph-shaped queries:

```zql
schema {
  type Team {
    name: String
    players -> Player[]
  }

  type Player {
    name: String
    salary: Int
  }
}

mutation csv ["./players.csv"] {
  Team(name: $Team) {
    players -> Player(name: $Name && salary: $Salary) { name salary }
  }
}

query {
  Team {
    name
    players -> Player(salary > 10000000) { name salary }
  }
}
```

Use `run_lang(schema, statement)` for one operation or `apply_zql(document)` for
an entire file. The [data-loading guide](docs/data-loading.md) covers raw sources,
JSON/CSV types and linking. The conformance corpus lives in
[zegadb/testsuite](https://github.com/zegadb/testsuite).

## How persistence works

Every write is appended to a CRC32-framed WAL (`wal.bin`) — with group
commit by default (5 ms / 64-entry batches) or fsync-per-write if you ask
for it. A statement is all-or-nothing: its writes go to the WAL as one entry,
and until the WAL accepts that entry no reader sees them; if the statement
fails or the WAL refuses it, nothing of it stays in memory or on disk.
Torn writes and bad checksums are detected and truncated on replay.
`Zega::snapshot()` writes a full `snapshot.bin`; the next open restores the
snapshot and replays only the WAL after it. Legacy WAL versions are migrated
automatically.

## Benchmarks

`zega-bench` runs the same workload against zega (embedded) and Neo4j
(Bolt) side by side: load 10,000 `User` nodes one write at a time, then
10,000 lookups by indexed `id`, reporting ops/s and p50/p99 latency.

```bash
cargo run --release -p zega-bench -- zega
cargo run --release -p zega-bench -- neo4j   # needs neo4j on localhost:7687
```

What changed in 0.2.0 (zega#55): the zega side used to run the same Cypher
text as Neo4j through zega's legacy query path, which is gone. It now runs
ZQL through `Zega::run_lang`, the call the server makes for `/zql`, with
`unique { User { id } }` standing in for Neo4j's index on `u.id`. ZQL has no
query parameters, so values are written into each statement's text, and
each call parses its schema; both costs are inside the measured time. The
Neo4j side is unchanged. The `commerce-bench` binary and the Cypher parity
tests against Neo4j were removed with the legacy language.

## Workspace layout

`zega` is the only published crate — the whole engine (query execution,
planner, JWT, policies, WAL, graph storage, ZQL parser and language) lives
inside it as private modules. Everything else in this workspace is a
consumer that depends on `zega` by path and is never published:

| Crate | What it is |
|---|---|
| `zega` | the database: the only thing on crates.io |
| `zega-server` | reusable ZQL HTTP service |
| `zega-cli` | `zega start` and the embedded `zega explorer` |
| `zega-wasm` | wasm-bindgen wrapper for the browser (in-memory) |
| `zega-bench` | the ZQL-vs-Neo4j benchmark |

## Status

zega is early (0.1.0). The engine, query language, WAL, server, and wasm
wrapper are functional and tested; the wire protocol is HTTP/JSON only (no
Bolt compatibility yet), and there is no REPL.

## License

Apache-2.0. See [LICENSE](LICENSE).

ZQL uses `@` for language-owned names and `&` for user edge fields. See
[Names in ZQL (APS 6)](docs/names.md) for the complete syntax inventory and migration rules.
