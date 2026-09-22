# zega browser

A four-pane workbench for the v2 schema language. The engine is compiled to
WebAssembly (`zega-wasm`), so there is no server to start. The graph lives
in the page and persists across reloads via `localStorage`.

Schema is the top-left pane. A read or a `mutation` is the top-right pane.
The bottom-left pane is the force-directed graph of everything stored. The
bottom-right pane is the JSON that came back.

## Run it

The prebuilt wasm package is committed, so this works with any static file
server:

```bash
cd browser
python3 -m http.server 8080
# open http://localhost:8080
```

Run executes the query pane. Ctrl/Cmd+Enter does the same. Seed library
writes the Le Guin example. Clear drops the saved database and reloads.
Writes are saved to `localStorage`.

The graph view's physics is [d3-force](https://github.com/d3/d3-force)
v3.0.0 (ISC license, © Observable), vendored as ESM in `vendor/` so the page
stays fully self-hosted — no CDN, no bundler.

## Rebuild the wasm package

Only needed after changing `zega-wasm` (or the engine crates):

```bash
cargo install wasm-pack   # once
cd zega-wasm
wasm-pack build --target web --out-dir ../browser/pkg
```

`zega-wasm` is its own workspace (see its `Cargo.toml`) so the wasm build
never tries to compile the server crates for `wasm32`.
