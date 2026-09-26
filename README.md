# zega

An embeddable graph database. Written in Rust.

<p align="left">
  <img src="image.png" alt="zega browser" width="70%">
</p>

zega provides a property graph and its own query language, ZQL, in
one engine, one binary, one dependency. It runs in-process in
any Rust application, persists through a write-ahead log with snapshots, and
compiles to WebAssembly for the browser.

No separate database server to install unless you want one. The server is a
single binary that speaks HTTP/JSON. But you can absolutely run it in the browser, 
just like [cqx](https://cqx.dev) does for running queries without a server:

<p align="left">
  <img src="cqx-zega-deka.png" alt="zega in the browser!" width="70%">
</p>

## Features

- **Property graph** — labelled nodes, typed relationships, property indexes,
  and variable-length traversals (`parent *1..3 -> Person`).
- **Path finding** — `road *path -> Junction(name: "B")` returns one route
  with its nodes, edges and cost: fewest edges, `by &km` (Dijkstra), or
  `by &km toward at` (A*, with `km: Float<km>`). See [paths](docs/path.md).
- **ZQL** — zega's own query language: an explicit schema, mutations that
  load JSON and CSV, and queries shaped like the graph they return. Filters,
  [paths](docs/path.md), [locations](docs/location.md),
  [vectors](docs/vector.md) and [discovery stages](docs/then.md) share one
  grammar.
- **Embeddable** — `zega` is a library first. Open a database with two
  lines of Rust and run graph queries against it.
- **WebAssembly** — `zega-wasm` exposes the same engine to JavaScript,
  in-memory, in the browser.
- **Durable** — CRC32-framed write-ahead log with group commit, torn-write
  detection, snapshots, and automatic WAL replay on open.
- **Optional server** — a tokio/axum HTTP server with token auth and
  ZQL execution on the blocking pool.

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
zega export graph.graph --data ./data
zega import graph.graph --data ./other --replace
```

`zega export` and `zega import` move a whole graph as one `.graph` file, the
format every zega surface reads and writes ([spec](docs/graph-format.md)).
Export streams and never leaves a partial file; `--schema s.zql` and
`--meta key=value` carry a schema and metadata along. Import is all or
nothing: a truncated or damaged file changes nothing. It refuses to replace a
database that holds data unless given `--replace`. An imported graph keeps its
file's schema text and metadata, so exporting it again gives the same file. `-` reads stdin or writes
stdout. Both take the data directory's lock, so they fail while a server holds
it: use `GET /graph` and `PUT /graph` then.

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
with an error status. `GET /health` returns `{ "ok": true }`. `GET /stats`
returns `{ "ok": true, "result": { "nodes": 3, "relationships": 1 } }`: the
graph's size, which Zega Cloud shows against a graph's plan.

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

`GET /graph` returns the whole graph as a `.graph` file
(`application/vnd.zega.graph`, [spec](docs/graph-format.md)); a client that
prefers `application/json` in its `Accept` header gets the JSON view the
explorer draws. `PUT /graph` replaces the graph with the `.graph` file in the
request body, all or nothing, and answers with what the file carried; uploads
over `--max-import-bytes` (default 64 MiB) get `413`, and one that stalls for
30 s gets `408`. Both spool through a staging file in the data directory, so
a slow client never holds the database; a download that stops reading for
30 s is dropped, and past 16 concurrent transfers the server answers `503`.
`DELETE /graph` replaces the graph with an empty one, dropping the schema and
metadata an import carried. The
explorer also uses the authenticated graph edit routes.
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

const file = db.exportGraph();          // Uint8Array, a .graph file
new ZegaWasm().importGraph(file);       // replaces that database's graph
```

`export_base64` / `import_base64` still work for the explorer's saved
sessions, but they carry the engine's internal snapshot and are deprecated
for anything else: use `exportGraph` / `importGraph`.

## Formatting ZQL and JSON

`zega fmt paths…` formats files or directories recursively (`*.zql` and `*.json`).
`zega fmt --check paths…` lists every file that would change and exits 1;
`zega fmt --stdin` reads source from stdin and writes the canonical layout.
Invalid or incomplete input is left unchanged. There are no style options.
[Formatting with zega fmt](docs/fmt.md) shows the layout rules on real
before/after examples.

The explorer uses the same formatter through WASM. Press ⌘S / Ctrl-S or
**Format** to format and save the active schema or query pane. Formatting is
undoable and preserves the cursor line (clamped when lines are removed).

The locked layout in [APS 12](https://github.com/zegadb/aps/issues/12) keeps
1–2 plain selection fields inline, opens schema types with 2+ fields, separates
types and top-level blocks with one blank line, and uses only `//` comments.
Display views keep 1–2 types inline and open 3+ types one per line. Per-type
attributes follow the same threshold inside parentheses; `globe(...)` settings
stay on the view's line. `String<url>` and `String<iso2>` keep their angle
brackets tight. Every `then` and `display { skip }` opens as a top-level
block; discovery sub-blocks stay compact when they fit, and boolean chains over
80 columns put each operand on its own line.
JSON objects with 1–2 members and scalar arrays stay inline when they fit 80
columns. JSON key order, number spelling and string escapes are preserved;
invalid input is returned unchanged. Directories include both `.zql` and `.json`.
Use `zega fmt --stdin --lang json` for JSON on standard input. The explorer uses
the same WASM formatter for its JSON import preview and result views.

## ZQL

ZQL is zega's own query language. A document declares a schema, loads data
with mutations, and asks queries shaped like the graph they return. It is not
Cypher, so it has its own name.

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
an entire file; the server runs the same language on `POST /zql`. The
[data-loading guide](docs/data-loading.md) covers raw sources, JSON/CSV types
and linking. Each core concept has its own page: [schema](docs/schema.md),
[mutations](docs/mutation.md), [queries](docs/query.md),
[conditions](docs/conditions.md), [relationships](docs/relationships.md),
[unique fields](docs/unique.md) and [errors](docs/errors.md). The conformance
corpus lives in
[zegadb/testsuite](https://github.com/zegadb/testsuite).

## How persistence works

Every write is appended to a CRC32-framed WAL (`wal.bin`) — with group
commit by default (5 ms / 64-entry batches) or fsync-per-write if you ask
for it. A statement is all-or-nothing: its writes go to the WAL as one entry,
and until the WAL accepts that entry no reader sees them; if the statement
fails or the WAL refuses it, nothing of it stays in memory or on disk.
Torn writes and bad checksums are detected and truncated on replay.
`Zega::import` keeps the imported `.graph` file in `graphs/` and commits it
with one WAL entry naming it, so a crash leaves the old graph or the new one,
never a mix. A checkpoint is the same thing for the database's own graph: it
writes the graph as a `.graph` file in `graphs/` and starts the WAL over with
one entry naming it plus the writes made since, so a restart reads that file
and replays only the WAL after it. A disk database checkpoints on its own once
the WAL reaches 16 MiB and the size of the graph it starts from (`zega start
--snapshot-every-mb`, `ZegaBuilder::snapshot_every`; 0 turns it off), and
`Zega::snapshot()` takes one now. While the graph is written out, in one pass
under the graph lock, every query waits, reads as well as writes: about 15-25
ms per MB of `.graph` file (under 0.1 s for 100,000 nodes, 1.5-2 s for
1,000,000 on an iMac). The sync and the WAL rotation run without the lock.
A crash at any point of a checkpoint reopens to every acknowledged write, and
a WAL that is lost or emptied after one is refused rather than opened empty.
Legacy WAL versions and `snapshot.bin` files are read and migrated
automatically. The first checkpoint marks the WAL version 3, as the first
`.graph` import does, so a zega from before `.graph` imports can no longer
open the directory: downgrading past that release is not supported.

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

`zega` is the only published crate — the whole engine (the ZQL parser,
checker and executor, WAL, and graph storage) lives
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

zega is early (0.2.0). The engine, query language, WAL, server, and wasm
wrapper are functional and tested; the wire protocol is HTTP/JSON only (no
Bolt compatibility yet), and there is no REPL.

## License

Apache-2.0. See [LICENSE](LICENSE).

NOTICE: the explorer's Flights sample (`browser/samples/flights*.csv`) contains
information from [OpenFlights](https://openflights.org/data.php), made
available under the Open Database License (ODbL 1.0); those files are a derived
database under the same licence, as are the Cities sample's routes
(`browser/samples/cities-routes.csv`). The Cities and Calgary samples' places
and cities, and the basemap, are © OpenStreetMap contributors, ODbL 1.0.

ZQL uses `@` for language-owned names and `&` for user edge fields. See
[Names in ZQL (APS 6)](docs/names.md) for the complete syntax inventory and migration rules.
