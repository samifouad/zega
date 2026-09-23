# Releasing zegadb

The unscoped npm package is **zegadb**. The name zega is unavailable (npm's
similarity filter matched egg); npm organizations do not own unscoped names.
The repository and Rust crate names remain zega.

## Build once, promote the tested payload

This follows [RFD 68](https://github.com/dekaruntime/rfd/issues/68) and dsc's
`tag-canary.yml`, `release.yml`, and `promote.yml`.

| Channel | Git / R2 prefix | npm version | npm dist-tag |
| --- | --- | --- | --- |
| canary | `v<V>-canary-<sha7>` | `<V>-canary.<sha7>` | `canary` |
| stable | `v<V>` | `<V>` | `latest` |

`Cargo.toml` keeps a plain stable workspace version. Every main push runs
`tag-canary.yml`: it refuses an already-promoted version or existing canary
tag, otherwise pushes the tag and explicitly dispatches `release.yml` against
it. GitHub's built-in token does not trigger workflows from its own tag pushes.
A user-pushed canary tag also triggers the build; a plain stable tag never does.

Release builds zega-server on Linux x64, macOS arm64/x64, and Windows x64. It
starts each resulting binary and executes an authenticated HTTP query. The
npm branch's builder compiles WASM once, stamps zegadb's canary version, packs,
and runs installed-tarball consumers in Node, Chromium, Vite, esbuild and Next.
The same WASM files and tested npm tarball, plus native binaries, are stored in
both `zega-releases/<CANARY>/` and `zega-wasm/<CANARY>/`. Each has a checksum
inventory in `manifest.json` and `release.json`. Readback is verified before
`canary/` and `canary.json` advance. Existing canary prefixes are immutable.

Sami promotes with **Actions → Promote → Run workflow → Branch: main → canary:
full tested tag → Run workflow**. Promotion resolves the tag's commit, refuses
an existing stable tag, and reads the canary's own `release.json` from R2.
Its commit must match the tag. Both bucket copies and every payload checksum
are checked before any writes. It copies objects to `v<V>/`, rewrites only
`version`, `tag`, `channel`, and `promoted_from` in both metadata files, verifies
readback, creates the stable tag at the canary commit, then advances `latest/`
and `latest.json` in both buckets. No compilation or dependency installation
occurs in the promotion job. The npm publishing job only installs the npm CLI.

**R2 payload artifacts are byte-identical between canary and stable.** The two
JSON bookkeeping files intentionally differ. Even R2's `package.tgz` remains
the original canary archive, with unchanged checksum. **npm tarballs differ by
their package.json version string.** Promotion repacks that R2 archive locally
for npm, preserving every other file byte and package field. It never uploads
the repacked archive over the original R2 payload or pretends the tarballs are
byte-identical. Tar/gzip container encoding can also change during repacking.

If a run stops after copying objects, before tagging, rerun the same canary;
readback gates still apply. If the stable tag was pushed but pointer updates
failed, do not delete or move the tag: Ava/Sami must inspect the recorded commit
and hashes and recover the pointers. Promotion deliberately refuses an existing
stable tag. A canary upload interrupted partway leaves a prefix that is refused
on rerun; inspect and remove only that incomplete prefix before retrying. npm
publishing failures can be retried using GitHub's **Re-run failed jobs**; never
rebuild or replace an already-published npm version.

## Environments and compilation

All PR jobs declare `public-ci`. All publishing and tagging jobs declare
`release`. Compilation uses the matrix's sccache bucket and a job-level
`RUSTC_WRAPPER: sccache`, including wasm-pack installation. The cache setup
fails closed on absent credentials or inaccessible bucket. Release compilation
also uses the limited public-ci cache identity; full R2 keys are used only by
copy/upload jobs. No PR job references full release credentials.

| Environment | Secret names |
| --- | --- |
| release | `R2_ACCOUNT_ID`, `R2_SCCACHE_ACCESS_KEY_ID`, `R2_SCCACHE_SECRET_ACCESS_KEY` (the all-buckets token) |
| public-ci | `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` (the sccache-only token) |

The endpoint is always composed from the account ID:
`https://${{ secrets.R2_ACCOUNT_ID }}.r2.cloudflarestorage.com`.
Cache buckets: `zega-sccache-{linux-x64,darwin-arm64,darwin-x64,windows-x64}`.
There is no stored npm credential or crates.io publisher.

## Sami's npm clicks, before enabling publication

Both npm jobs have a literal `if: ${{ false }}`. Keep them disabled until Sami
has configured trust. Existing npm settings from the old npm branch, if any,
need replacing: the package is zegadb, the environment is release, and there
is no longer an npm-release.yml workflow.

1. Sign in to npmjs.com → profile → **Packages** → **zegadb** → **Settings** →
   **Trusted publishing** → **Add trusted publisher** → **GitHub Actions**.
   The package must already exist under Sami's user account. If it does not,
   stop at this step and resolve package creation with npm; this task does not
   bootstrap by publishing a placeholder or adding credentials.
2. Add a connection with **Organization or user: zegadb**, **Repository: zega**,
   **Workflow filename: release.yml**, **Environment name: release**. Under
   **Allowed actions**, allow direct **npm publish**. Save and complete 2FA.
3. Add another connection with the same fields except **Workflow filename:
   promote.yml**. This covers stable publication. npm currently supports
   multiple trusted publishers. OIDC's caller identity on the automatically
   dispatched canary path must be confirmed by the first real publish; if npm
   reports `tag-canary.yml` as caller, configure that exact additional identity
   in the same release environment before retrying the failed npm job.
4. **Settings → Publishing access → Require two-factor authentication and
   disallow tokens → Update Package Settings**.
5. Ava reviews a PR removing the two literal false guards only after that setup.
   npm 11.16.0 (>=11.5.1), Node 24 and `id-token: write` are already wired in
   both GitHub-hosted publishing jobs. The next main merge creates a canary.
6. Test `npm install zegadb@canary`. After acceptance use the Promote button
   above. Confirm `npm install zegadb@latest`, provenance, versions, and R2
   readback hashes on that first actual release.

The field names, npm requirements and multiple-publisher support were checked
against [npm's trusted-publisher documentation](https://docs.npmjs.com/trusted-publishers/).
Saving trust settings does not verify an OIDC publish. This change performs no
npm/crates.io publication, live canary tagging, stable promotion or R2 writes.

## Reproduce local checks

Use Node >=22.14 (CI 24), Rust 1.96.0, wasm-pack 0.15.0 and sccache. On this Mac,
keep the globally configured Rust wrapper and use a private build directory:

```sh
export CARGO_TARGET_DIR="$PWD/.target" TMPDIR="$PWD/.tmp"
export npm_config_cache="$PWD/.tmp/npm-cache"
mkdir -p "$TMPDIR"
chmod 700 "$TMPDIR"
python3 -m unittest -v scripts.test_channels
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --all-features
npm ci --ignore-scripts --no-audit --no-fund
npm run build
node npm/stamp-version.mjs "0.1.0-canary.$(git rev-parse HEAD | cut -c1-7)"
npm run pack:package
# macOS: use installed Chrome; CI installs Playwright's Chromium.
export PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
npm test
npm run test:exports
```

Keep zega-wasm's standalone workspace: its committed lockfile and path
dependencies isolate browser builds from the native server workspace.
The release workflow runs builds on PR events without publishing. The
Tag canary and Promote proof jobs exercise their production scripts against
isolated local Git/R2 fixtures and print both refusal messages and the integrity
failure. They are not live-bucket or live-tag tests.
