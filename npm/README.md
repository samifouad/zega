# zegadb

An embeddable graph database with ZQL v2. One package contains
the JavaScript API, TypeScript declarations and WebAssembly engine. It has no
runtime npm dependencies or install scripts. Requires an ES module environment
and, on the server, Node 22.14 or later.

```sh
npm install zegadb
```

```js
import { createDatabase } from 'zegadb';

const db = await createDatabase();
try {
  const result = db.run(
    'type Person { name: String }',
    'mutation { Person(name: "Ada") { name } }',
  );
  console.log(JSON.parse(result)); // { name: 'Ada' }
} finally {
  db.free();
}
```

The same code works in Node and Vite. Initialization is asynchronous and shared;
each call creates an independent database. Queries are synchronous. In CommonJS,
use `await import('zegadb')`. For large workloads use a Web Worker or worker thread.

## Exports

- `zegadb`: `createDatabase(options?)`, `ZegaWasm`, and the `DatabaseOptions` and
  `InitInput` types. `ZegaWasm` is the generated class; initialize via
  `createDatabase()` before constructing it directly.
- `zegadb/wasm`: the unmodified wasm-bindgen API and declarations, including
  default async `init` and `initSync`, for callers managing initialization.
- `zegadb/zega_wasm_bg.wasm`: the binary asset for bundler loaders or custom hosting.

`db.run(schema, source)` executes ZQL v2 against a schema string.
`db.apply(source)` executes a complete `.zql` file containing schema, mutation
and query blocks. `db.check(schema, source)` returns JSON diagnostics.
`db.query(source, paramsJson)` exposes the older Cypher-style query interface.
Results from these methods are JSON strings; use `JSON.parse`.
The generated types also cover graph inspection, node/relationship deletion,
connections and base64 snapshot import/export. Errors from the bindings can be strings, so catch `unknown`.

The database is in memory in both environments. Persist explicitly using
`export_base64()` and `import_base64()`. Call `free()` when finished and do not
use a freed handle. Snapshot restore replaces the receiving database's state.

## Bundlers and WASM assets

The conditional exports select the browser loader for browser builds and the
filesystem loader for Node. Both load the same binary. The browser loader uses
`new URL('./wasm/zega_wasm_bg.wasm', import.meta.url)`, which Vite and Webpack
can emit as an asset. Serve it as `application/wasm`. Vite needs no WASM plugin.
Generated glue is explicitly retained in `sideEffects`; do not override that
metadata with blanket tree-shaking settings.

With Next (including Turbopack), call `createDatabase()` in a client component's effect or event
handler, not during rendering. A server import selects the Node loader. For a
bundler that does not emit `new URL` assets (including esbuild), use its asset
loader or copy the exported binary to your public assets:

```js
// esbuild: loader: { '.wasm': 'file' }, publicPath matching your asset server
import wasmURL from 'zegadb/zega_wasm_bg.wasm';
import { createDatabase } from 'zegadb';
const db = await createDatabase({ wasm: wasmURL });
```

`options.wasm` accepts a URL/string, Response, bytes, compiled WebAssembly.Module
or a promise of one. It controls the first initialization only; later calls use
the already loaded module. Node's default reads the packaged bytes directly
because Node fetch cannot read `file:` URLs. Importing the package itself does
not initialize WASM. This supports custom asset hosting and SSR imports.

## Scope

This package embeds the engine. A remote HTTP client for a `zega-server` server
belongs in a later, separately scoped client package once the remote protocol
is stable: it needs authentication, network errors, cancellation and protocol
version handling, and remote-only users should not download a local engine.
That future client would not split these tightly coupled JS/WASM engine files.
The repository's own explorer remains in `browser/` and builds from Rust directly.

Source and issues: [zegadb/zega](https://github.com/zegadb/zega).
