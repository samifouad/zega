# zega canary and stable handoff

Checkout: `/Volumes/Projects/codex/zega-channels`
Branch: `codex/channels`
Base: main `718349d`, with npm branch `63de5ba` merged locally as `9e39ec5`.

## Landing order and ownership

Ava owns review and merging. Do not push this branch to main directly.
`codex/npm` remains untouched: its existing builder, package API and installed
consumer tests are reused through a merge commit, not reimplemented. Ava should
land `codex/npm` first, then `codex/channels` with merge commits. The second merge
renames the emitted package/imports to **zegadb**, retires the old disabled
npm-release.yml publisher, and replaces its GHA cache wiring with the required
R2 environment/matrix setup. Both branches keep the original npm publisher
disabled throughout landing. Alternatively Ava can review this combined branch
as the integration PR; it already contains the npm commit as an ancestor.

This task changes no Rust implementation and adds no secrets, npm credentials,
crates.io publisher, live release tags, live R2 objects or main pushes.
The two npm publishing jobs are hard-disabled by literal `if: ${{ false }}`.

## Delivered

Read the existing dsc checkout's three workflows and RFD 68 before writing.
`tag-canary.yml` derives `v<V>-canary-<sha7>`, refuses promoted versions and
duplicates, pushes the tag, then dispatches release.yml at that ref (required
because GITHUB_TOKEN tag pushes do not themselves start another workflow).

`release.yml` builds/test-packs zegadb `<V>-canary.<sha7>`, builds native servers
for Linux x64, macOS arm64/x64 and Windows x64, checks metadata and payload hashes,
uploads to both named R2 buckets and verifies readback before moving canary.
Native artifacts are exercised through authenticated /health and /cql requests.
Stable tags cannot start builds. Existing immutable canary prefixes are refused.

`promote.yml` only accepts an explicitly dispatched canary on main. Its script
checks the canary tag's workspace version and commit suffix, refuses existing
stable tags, compares the R2 canary release.json commit with the tag's commit,
and verifies both buckets' complete payload inventories before any writes.
It copies to `v<V>/`, rewrites only version/tag/channel/promoted_from in both
metadata files, verifies copied bytes, tags the canary commit and moves latest.
No Rust/WASM/JavaScript rebuild occurs during promotion.

**R2 payload artifacts are byte-identical; manifest.json and release.json are
bookkeeping exceptions. npm tarballs differ by package.json's version string.**
Repacking may also change tar/gzip container encoding; all other extracted file
bytes and manifest fields stay equal. R2's copied package.tgz deliberately stays
the original canary tarball; the stable npm tarball is a separate Actions artifact.

Every PR job declares public-ci; every publishing/tagging job declares release.
Rust compilation uses job-level RUSTC_WRAPPER and matrix-selected sccache buckets.
Even release builds use the restricted cache identity; only release upload/copy
jobs use full R2 credentials. No endpoint secret is read: endpoints are composed
from R2_ACCOUNT_ID. Cache and release credentials fail closed without values in logs.

## Local proof

- `cargo check --locked --workspace --all-targets`: pass, no warnings.
- `cargo clippy --locked --workspace --all-targets --all-features`: pass, no warnings.
- `cargo build --locked --release -p zega-server` and built-binary HTTP smoke: pass.
- npm build/pack and installed canary consumers: pass in Node, Chromium,
  Vite development/production, esbuild, and Next server/client; types pass in
  NodeNext and Bundler modes.
- Broken exports fail in Node and browser; restored exports pass again.
- Repacked stable npm tarball passes the same installed Node/browser/bundler/Next consumers; no rebuild.
- Nine production-channel tests pass isolated Git/R2 fixture tests. Actual output:

  ```text
  REFUSED: v1.2.3 is already promoted; bump the workspace version
  REFUSED: canary tag v1.2.3-canary-<fixture-sha> already exists; nothing to do
  ERROR: release.json commit mismatch: tag ... points at ..., artifacts record ffffffffffffffffffffffffffffffffffffffff
  PROMOTED: R2 payloads byte-identical; npm contents differ only by package.json version
  ```

  Refusals assert no pushed refs, dispatch, or R2 mutation. The mismatch stops
  after the first release.json read, before copying payloads. Positive promotion
  runs with a newer HEAD and still tags the canary commit. A corrupted WASM
  payload is also rejected. Canary assembly checks the separate WASM objects
  equal the tested npm archive's bytes.
- Removing each of the promoted-version, duplicate-tag and commit checks from
  a temporary script makes its corresponding proof fail. The production source was unchanged by mutation probes.
- actionlint passes with only the intentional literal-false diagnostic excluded.
- WASM compilation reports the 10 existing native/WASM conditional warnings
  inherited from the npm branch; no new Rust warnings introduced.

Logs are in `.tmp/`: cargo-check.log, cargo-clippy.log, native-build.log,
native-smoke.log, npm-build.log, npm-pack.log, npm-test.log, npm-negative.log,
channel-proofs.log, npm-stable-test.log, and mutation-{promoted,duplicate,integrity}.log.

## GitHub run evidence

Channel implementation commit: `bca82a9f14ef5997a95ec61fc8edbfc1bc4d9e91`.
Repository commits use Sami Fouad <sfouad@gmail.com>, have SSH signature
headers, and contain no agent attribution. Main and codex/npm were not pushed.

- [Tag canary — success](https://github.com/zegadb/zega/actions/runs/35805409630):
  both required refusal messages printed; three tag tests plus two release
  assembly/ref-validation tests passed. Real tag job skipped on the branch.
- [Promote — success](https://github.com/zegadb/zega/actions/runs/35805409624):
  four promotion proofs passed. The production script printed the release.json
  commit mismatch and exited nonzero before any writes; the test asserted it.
  Actual promotion and npm jobs skipped on the branch.
- [Explorer — initial failure](https://github.com/zegadb/zega/actions/runs/35805409602):
  existing browser build tried to copy nonexistent browser/LICENSE. Fixed to
  copy ../LICENSE. Local explorer build passes and its emitted license exactly
  matches the repository license. Both existing Explorer Playwright tests also
  pass locally: real editor/Run button/WASM graph flow and HTTP MIME checks.
  No product behavior changed.

Workflow logs are saved in `.tmp/github-{tag-proof,promotion-proof,explorer-failed}.log`.
The real npm archives also pass a byte comparison: only package/package.json
changes, and only its version field; the other nine members are identical.
See `.tmp/npm-byte-proof.log` for archive hashes.

## Credential blocker — needs Sami

Read-only GitHub secret-name inspection found the configured names reversed:

| Environment | Observed names (no values read) | Required correction |
| --- | --- | --- |
| release | R2_ACCOUNT_ID, R2_ENDPOINT, R2_SCCACHE_ACCESS_KEY_ID, R2_SCCACHE_SECRET_ACCESS_KEY | Needs Sami: R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY on zegadb/zega/release |
| public-ci | R2_ACCOUNT_ID, R2_ENDPOINT, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY | Needs Sami: R2_SCCACHE_ACCESS_KEY_ID and R2_SCCACHE_SECRET_ACCESS_KEY on zegadb/zega/public-ci; remove the full release keys |

Sami: GitHub **zegadb/zega → Settings → Environments → release**, correct the
release credential pair; then **public-ci**, remove full release keys and
configure the cache-only pair. Keep R2_ACCOUNT_ID in both. These actions belong
to Sami; no secret was created, copied, retrieved or printed by this task.
The existing R2_ENDPOINT names are unused by the channel workflows.

Credential-dependent runs are paused. Branch-push proof jobs are secret-free;
they exercise the production scripts using fixture-only Git repositories and
an R2 CLI simulator. Real native matrix/cache jobs start on PRs and real canary
release events after this environment correction. Do not open the PR while
public-ci contains the full release keys.

## npm setup and real-release proof still needed

Exact click sequence is in [npm/PUBLISHING.md](npm/PUBLISHING.md).
Once the unscoped package zegadb exists under Sami's user account:

1. npmjs.com → profile → Packages → **zegadb** → Settings → Trusted publishing
   → Add trusted publisher → GitHub Actions.
2. Set owner **zegadb**, repository **zega**, workflow **release.yml**,
   environment **release**, allow direct **npm publish**, save/2FA.
3. Add the second connection for **promote.yml**, otherwise identical. The
   automatically dispatched canary's OIDC caller identity needs confirmation
   on the first real publication; npm may require the tag-canary.yml caller
   connection too. Both publishing jobs use npm 11.16.0 and id-token: write.
4. Settings → Publishing access → **Require two-factor authentication and
   disallow tokens** → Update Package Settings.
5. After Sami configures trust, Ava reviews a change removing both literal
   false guards. Only then can a main merge publish a canary under `canary`.
6. Sami tests that canary, then GitHub Actions → **Promote** → Run workflow →
   branch **main**, type the full canary tag, Run workflow. Stable npm uses `latest`.

If zegadb does not yet exist, package creation is still Sami-held; no placeholder
or token bootstrap was attempted. The unscoped name zega cannot be used because
npm's similarity filter matched egg; creating an npm organization does not
reserve or own an unscoped name.

Unproven until actual infrastructure/release execution: live R2 permissions and
readback/copy behavior, all four hosted native builds and cache writes, automatic
main→tag→dispatch behavior, live stable tag/pointer updates, npm OIDC exchange,
registry provenance and actual canary/latest installs. Fixture proofs do not
claim to establish those facts. No npm or crates.io publication occurred.

-codex
