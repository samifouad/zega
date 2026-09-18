# zega browser

A Neo4j-Browser-style workbench for zega that runs **entirely in the
browser** — the database engine itself is compiled to WebAssembly
(`zega-wasm`), so there is no server to start, nothing to install, and the
whole graph lives in the page. Your data persists across reloads via
`localStorage`.

## Run it

The prebuilt wasm package is committed, so this works with any static file
server:

```bash
cd browser
python3 -m http.server 8080
# open http://localhost:8080
```

## Features

- **Editor + result frames** — run ZQL with the Run button or Ctrl/Cmd+Enter;
  every query becomes a frame (newest on top) that can be re-run, collapsed,
  downloaded as JSON, or dismissed.
- **Graph / Table / Text views** per frame, with a results overview
  (per-label node counts, per-type relationship counts) like Neo4j's.
- **Graph view** — force-directed layout, drag nodes, pan/zoom the canvas,
  hover a node for its properties. Nodes are colored by label; relationship
  type labels sit on the edges.
- **Sidebar** — live database info: node counts, node labels and
  relationship types (click to scaffold a `MATCH`), plus query history.
- **Sample graph** — one click loads a small movie graph
  (Person/Movie, ACTED_IN/DIRECTED) and opens a graph view of it.
- **Persistence** — every write query autosaves the database
  (`export_base64`) to `localStorage`; reload and your data is still there.
  `export`/`import` in the top bar move snapshots as files; `clear` wipes.
- **KV commands** work too — `SET KEY foo = "bar"`, `GET KEY foo`, lists,
  TTLs; it's the same engine.

## Rebuild the wasm package

Only needed after changing `zega-wasm` (or the engine crates):

```bash
cargo install wasm-pack   # once
cd zega-wasm
wasm-pack build --target web --out-dir ../browser/pkg
```

`zega-wasm` is its own workspace (see its `Cargo.toml`) so the wasm build
never tries to compile the server crates for `wasm32`.
