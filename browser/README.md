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

## Native CLI mode

`zega explorer --data ./data` serves the same static application embedded in the
binary at `http://127.0.0.1:9343`. It opens nothing automatically. The CLI provides
`/explorer-config.json`, selecting the native `/zql` and `/graph` backend. File
picker imports still pass raw text to Rust; normal ZQL URL/path loads use the
native library transport. Opening or reloading the UI never seeds the database.
The standalone deployed site remains a wasm/localStorage database.

After `cargo build --locked -p zega-cli` from the workspace with
`CARGO_TARGET_DIR=.target`, run `npm run test:cli` here for the real Chromium
native-backend import/reload/restart test. `assets.json` is the shared static
bundle inventory used by both `npm run build` and the CLI's build script; a
normal Cargo build needs no Node installation. The build validates the vendored
wasm hashes before embedding the bundle.

## explorer2: remote mode

`npm run build:remote` (the same as `npm run build -- --remote`) writes
`dist-remote/`: the same bundle, with `remote/backend.js` as its `backend.js`.
It speaks the native backend's protocol (`native.js` is `backend.js` with its
class exported) to a remote zega server at `https://zega-explorer2-api.fly.dev`
(`--remote-base=URL` for another): one shared-cpu-1x/256 MB Fly Machine with
its graph on a volume, stopped by Fly's proxy when idle and started by the next
request (`zega-cloud/explorer2-api/fly.toml`). The access token is never in the
bundle: the page asks for it, keeps it in `localStorage`, sends it as a Bearer
header, and "forget token" drops it. The bar under the top bar shows each
request's round trip and the running median, says when the Machine is waking,
and loads the Calgary sample into an empty graph. `npm run build` and `dist/` are
unchanged. `wrangler.explorer2.jsonc` serves `dist-remote/` as the
`zega-explorer2` Worker on explorer2.zega.dev.

`npm run test:remote` runs it against a local `zega start --token-file` behind
a stand-in for the Fly Machine (`remote-tests/stand-in.js`); build the
CLI first, as for `npm run test:cli`.
