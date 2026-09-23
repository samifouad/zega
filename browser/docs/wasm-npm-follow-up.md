# Follow-up: publish wasm with the engine

[RFD 2](https://github.com/zegadb/rfd/issues/2) is defining the `@zega/*` npm
surfaces. The explorer currently has **no npm engine dependency**. The proposed
name below is `@zega/wasm`; it is a future package, not something this extraction
installs or claims has been published. Confirm the final name in RFD 2 first.

Vendoring `pkg/` is the immediate choice: it preserves the exact working engine
artifact and makes normal builds independent of Rust and the engine checkout.
The checksummed source manifest makes deliberate updates reviewable, but cannot
tell us that upstream has released a newer engine. A submodule or build-time
Git fetch would still require the separate wasm workspace, Rust, wasm-pack and
engine access on every clean build. Neither gives us a published versioned
artifact, and neither is needed for this static application.

## Work in zegadb/zega

1. Set `zega-wasm`'s package version to the engine release version and keep them
   in sync in the engine's version-bump procedure. Fix its repository metadata
   to `https://github.com/zegadb/zega`. Commit `zega-wasm/Cargo.lock` and pin the
   Rust and wasm-pack versions used by the release job.
2. Add an engine release build for the standalone crate, not the root workspace:

   ```sh
   rustup target add wasm32-unknown-unknown
   wasm-pack build ./zega-wasm --target web --release --out-dir ./pkg --locked
   ```

   `--out-dir ./pkg` is relative to the crate, so output is `zega-wasm/pkg`.
   In release CI, configure `RUSTC_WRAPPER: sccache` at job level, install
   sccache, and fail if it is missing, per the workspace rules.
3. Normalize the generated package name and public surface before packing
   (these commands assume RFD 2 accepts `@zega/wasm`):

   ```sh
   node --input-type=module - <<'JS'
   import { readFileSync, writeFileSync } from 'node:fs';
   const file = 'zega-wasm/pkg/package.json';
   const pkg = JSON.parse(readFileSync(file, 'utf8'));
   pkg.name = '@zega/wasm';
   pkg.repository = { type: 'git', url: 'https://github.com/zegadb/zega', directory: 'zega-wasm' };
   pkg.files = ['*.js', '*.wasm', '*.d.ts', 'snippets', 'LICENSE', 'README.md'];
   pkg.exports = {
     '.': { types: './zega_wasm.d.ts', import: './zega_wasm.js' },
     './zega_wasm_bg.wasm': './zega_wasm_bg.wasm'
   };
   pkg.publishConfig = { access: 'public' };
   writeFileSync(file, JSON.stringify(pkg, null, 2) + '\n');
   JS
   cp LICENSE zega-wasm/pkg/LICENSE
   cd zega-wasm/pkg
   npm pack --dry-run
   npm pack
   ```

   Keep the version generated from the standalone Cargo package; assert it
   equals the engine release version. Include both declaration files, all JS
   glue/snippets and the wasm. `--target web` preserves the relative wasm URL
   used by the explorer; do not switch to the bundler target.
4. Install the actual packed tarball in a disposable consumer, copy its files
   to the explorer's `dist/pkg`, and run the existing headless browser test.
   Assert it uses the new version, runs ZQL, and serves wasm as
   `application/wasm`. A successful Rust build alone is insufficient.
5. Sami handles npm scope/account setup and the first authorized publication.
   Add engine release publication using npm's supported trusted-publisher
   setup for that exact repository/workflow/environment. Use a GitHub-hosted
   runner, Node >=22.14.0, npm >=11.5.1, and job permissions `contents: read`
   and `id-token: write`. Release CI packs
   first and publishes the validated tarball with
   `npm publish <tarball> --access public --provenance`. Match package version
   to engine version and fail if it already exists. No token should be copied
   from another repo, and the explorer workflow gets no publishing secrets.
6. After publication, verify `npm view @zega/wasm@<engine-version> dist.integrity`,
   install that exact version into a fresh consumer, and repeat the browser
   query test against the bytes retrieved from npm.

## Work in zegadb/explorer after publication succeeds

1. Run `npm install --save-exact @zega/wasm@<engine-version>` and commit the
   package manifest and lockfile. Use a real published engine version.
2. Change `scripts/build.mjs` to copy the installed package's runtime assets
   from `node_modules/@zega/wasm/` to `dist/pkg/`. Keep glue and wasm adjacent;
   the browser imports stay `./pkg/zega_wasm.js` and need no import map or CDN.
   Include any generated `snippets/` in the copy.
3. Replace the vendored-file integrity check with verification of the installed
   package version and required files; retain compilation and browser checks.
   `npm ci` then supplies the package using the committed lockfile integrity.
4. Remove the tracked `pkg/`, `wasm-source.json` and local rebuild helper only
   in that migration PR. Update the static-server instructions to serve
   `dist/` after `npm ci && npm run build`.
5. Run `npm ci`, `npm run build`, and `npm test` from a fresh explorer-only
   checkout. Prove both loading and a real ZQL result again. Future engine
   releases update the exact dependency and lockfile in reviewed PRs.

None of these release, npm-account, publication or engine-repository changes
are part of the extraction branch.

Reference: [npm trusted publishing](https://docs.npmjs.com/trusted-publishers/).
