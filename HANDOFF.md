# zega npm engine handoff

- Checkout: `/Volumes/Projects/codex/zega-npm`
- Branch: `codex/npm`
- Base: `grok/v2-repl-10` at `d3fa695` (not `main`)
- Package: `zega@0.1.0`
- Tarball: `/Volumes/Projects/codex/zega-npm/artifacts/zega-0.1.0.tgz`

Nothing was published, tagged, merged, or pushed to main. The release workflow
is deliberately disabled. Review/merge remains with Ava. No npm credential was
requested, created, read, copied, or configured.

## Delivered

`npm run build` runs wasm-pack 0.15.0 against `zega-wasm/` with `--target web
--release --locked`, then assembles `dist/`. It derives npm metadata/version
from `workspace.package` in root `Cargo.toml`; the root npm tooling package is
private and has no independent version. The standalone WASM version must match
the workspace or the build fails. The benchmark crate now also inherits the
workspace version.

The public engine API is `createDatabase(options?)` and the generated
`ZegaWasm` class. `zega/wasm` exposes the original bindings and `init`/`initSync`;
`zega/zega_wasm_bg.wasm` exposes the asset for bundlers. The wrapper and generated
WASM declarations ship. Query results retain the bindings' JSON-string contract.
The engine is in memory in Node and the browser, with explicit snapshot export,
restore and `free()` lifecycle. See [the package README](npm/README.md).

One WASM binary serves both environments. Conditional exports choose a browser
URL loader or a Node filesystem loader. The generated glue is retained by the
`sideEffects` allowlist. Vite and Next emit the URL asset; esbuild uses its file
loader and the documented `wasm` initialization option. There are no runtime
npm dependencies, install scripts, native addons or second engine package.

A remote HTTP client belongs in a later separate client package: network/auth
and protocol compatibility differ from in-process execution, and remote-only
users need not download WASM. This does not split the coupled JS/WASM engine.

**Keep `zega-wasm` excluded from the root Cargo workspace.** Its explicit
standalone workspace was introduced in `f8b8ce5` to isolate WASM from server
builds. Targeted builds could work in a combined workspace, but changing
workspace-wide targeting/feature resolution and locks adds no benefit here.
The dedicated lockfile and path dependency on the current core already work.

`npm-ci.yml` runs on every push and PR, uses no secrets and never publishes.
It builds/packs, installs the tarball into independent consumers, runs queries
in Node and real Chromium, checks types, and runs the broken-exports proof.
Rust compilation in CI requires job-level sccache. Action revisions, npm dev
dependencies, wasm-pack and Rust are pinned.

`npm-release.yml` accepts only tag pushes, checks the exact stable workspace
version, and publishes the tested tarball from a separate GitHub-hosted OIDC
job. The upstream build job has literal `if: ${{ false }}`, so neither it nor
the dependent publisher can run. No dispatch trigger or token fallback exists.

## Tarball inventory

`npm run pack:package` ran real `npm pack` and checked the complete allowlist.
Every path below is beneath `package/` in the tar archive:

| File | Bytes | Purpose |
| --- | ---: | --- |
| `package.json` | 1,122 | Name/version, correct repository, conditional exports, asset/side-effect metadata |
| `browser.js` | 448 | Async browser initialization and independent database creation |
| `node.js` | 551 | Same API, reading the packaged WASM through Node filesystem APIs |
| `index.d.ts` | 498 | Handwritten factory/options declarations and generated type re-exports |
| `wasm/zega_wasm.js` | 20,825 | wasm-bindgen generated JS glue |
| `wasm/zega_wasm.d.ts` | 4,470 | Generated class and initialization declarations |
| `wasm/zega_wasm_bg.wasm` | 998,715 | The single compiled engine |
| `wasm/zega_wasm_bg.wasm.d.ts` | 2,026 | Generated binary export declarations |
| `README.md` | 4,068 | Consumer API and bundler instructions |
| `LICENSE` | 11,341 | Apache-2.0 license |

10 files, **394,309 bytes packed**, **1,044,064 bytes unpacked**. No source Rust,
test fixtures, build tools, explorer files, second manifest, caches or secrets.

SHA-256: `e89cd120d95aa38ec48a006c1ff2d4ce576ee13b73aeb80792e1e640b9a3217a`

## Actual consumer output

Every consumer installs this tarball with npm. None imports the source tree or
uses a workspace symlink. Each executes ZQL v2:

```js
db.run('type Person { name: String }', 'mutation { Person(name: "Ada") { name } }')
```

Captured output:

```text
node: {"name":"Ada"}
browser/vite: {"name":"Ada"} (WASM HTTP 200, application/wasm)
browser/vite-dev: {"name":"Ada"} (WASM HTTP 200, application/wasm)
browser/esbuild: {"name":"Ada"} (WASM HTTP 200, application/wasm)
next/server: {"name":"Ada"}
browser/next: {"name":"Ada"}
```

The Node fixture also reads the mutation back, checks independent instances and
restores a snapshot. Browser checks assert the result, rather than just a
successful build, and Vite/esbuild verify a real HTTP WASM request under
`/consumer/`. Next 16.3.6 uses its default Turbopack production build and checks
both a server route and a client effect. TypeScript passes with `NodeNext` and
`Bundler` resolution against the installed declarations. Local runtime:
Node 26.3.1, npm 11.16.0, installed Chrome via Playwright; CI is configured for
Node 24 and Playwright Chromium. GitHub CI was not polled or claimed green.

## Verify by reverting

`npm run test:exports` points `exports["."]` at missing `./missing-entry.js`,
packs/installs the broken package, and observes both consumers fail:

```text
broken exports/node: exit 1; Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/Volumes/Projects/codex/zega-npm/.tmp/consumers/node/node_modules/zega/missing-entry.js' imported from /Volumes/Projects/codex/zega-npm/.tmp/consumers/node/index.mjs
broken exports/browser: Vite failed to resolve import "zega"
Restored original exports and repacked.
node: {"name":"Ada"}
browser/vite: {"name":"Ada"} (WASM HTTP 200, application/wasm)
browser/vite-dev: {"name":"Ada"} (WASM HTTP 200, application/wasm)
browser/esbuild: {"name":"Ada"} (WASM HTTP 200, application/wasm)
```

Restoration is in `finally`, and the delivered tarball has the valid exports.

Other completed checks:

- Clean `npm ci --ignore-scripts --no-audit --no-fund`, build, pack and consumer checks.
- `cargo check --locked --workspace --all-targets`: exit 0, no warnings.
- `cargo clippy --locked --workspace --all-targets --all-features`: exit 0, no warnings.
- WASM build: exit 0, with 10 existing unused-import/constant/variable/field
  warnings in unchanged native/WASM conditional code; no new warnings introduced.
- `actionlint -ignore 'constant expression "false"' ...`: pass. The sole
  ignored diagnostic is the intentional literal release disable.
- Release gate accepts `push` + `refs/tags/v0.1.0`; rejects main branch,
  `v0.2.0`, and `workflow_dispatch`, each with exit 1.
- `git diff --check`: pass. `git diff --name-only -- browser/`: empty.

Local logs are under `.tmp/`: `npm-build.log`, `npm-pack.log`, `npm-test.log`,
`npm-next-server.log`, `npm-negative.log`, `cargo-check.log`, `cargo-clippy.log`.
Reproduction commands and all registry/GitHub setup instructions are in
[npm/PUBLISHING.md](npm/PUBLISHING.md).

## Still blocked on Sami

1. Create the `zega` npm organization if desired. This creates `@zega`; npm
   organizations cannot own the **unscoped** `zega` package. That package needs
   user-account ownership. The public registry returned HTTP 404 for `zega`
   during this run; that is absence, not a reservation or guaranteed name grant.
2. Resolve first-package registration under the strict OIDC-only/no-token
   policy. npm's documented trust setup requires an existing package, and the
   npm PM's September 17 roadmap still describes automatic first-package
   creation as planned. No token or manual-bootstrap workaround was attempted.
   The workflow stays disabled pending a supported token-free route.
3. When the package exists, use the exact npmjs.com click path in
   [npm/PUBLISHING.md](npm/PUBLISHING.md): configure GitHub owner `zegadb`, repo
   `zega`, workflow `npm-release.yml`, environment `npm`, allow direct publish,
   and disallow token publishing. Configure the protected GitHub `npm`
   environment; Ava may then review enabling the workflow and Sami may tag.
4. Resolve the conflicting metadata instruction: `browser/pkg/package.json:8`
   still points to the previous repository owner. The explicit **do not
   touch `browser/`** rule was preserved while the metadata-only exception
   question remained unanswered. Every occurrence outside that frozen tree
   was corrected, and the new npm tarball advertises `zegadb/zega` correctly.

-codex
