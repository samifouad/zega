# Building and releasing `zega`

No package has been published by this change. `npm-release.yml` has a literal
`if: ${{ false }}` on its build job; its dependent publisher cannot run.
Only Ava may merge/enable the workflow, and only Sami may promote a release.

## Local build and consumer checks

Use Node >=22.14 (CI uses 24), Rust 1.96.0 with `wasm32-unknown-unknown`,
wasm-pack 0.15.0, and sccache. From the repository root:

```sh
export CARGO_TARGET_DIR="$PWD/.target" TMPDIR="$PWD/.tmp"
export npm_config_cache="$PWD/.tmp/npm-cache"
export PLAYWRIGHT_BROWSERS_PATH="$PWD/.tmp/playwright"
mkdir -p "$TMPDIR"
chmod 700 "$TMPDIR"
npm ci --ignore-scripts --no-audit --no-fund
npx --no-install playwright install chromium
npm run build
npm run pack:package
npm test
npm run test:exports
```

On the iMac, the installed Chrome can be used instead of downloading Chromium:
`export PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'`.
Fixtures live in `npm/consumers/`; the checks copy them into `.tmp/`, install the
tarball there, and test the installed package. There are no workspace links or
imports from `dist/` in consumer code. Artifacts are ignored by Git.

`npm run build` reads `workspace.package.version` from root `Cargo.toml` and
generates `dist/package.json`. It refuses a mismatched standalone WASM crate
version. On a version bump, mirror the workspace version into
`zega-wasm/Cargo.toml` and update its committed lockfile before building; do not
version npm independently. All local crates and the npm package are currently
0.1.0. `npm pack` runs only against `dist/`; the root tooling package is private.

Keep `zega-wasm` excluded from the root Cargo workspace. Commit `f8b8ce5`
introduced its standalone workspace specifically to isolate browser builds
from native server targets. A targeted `cargo build -p` could select WASM in a
shared workspace, but adding it would also change workspace-wide build/feature
resolution and locking for no packaging benefit. The dedicated WASM lockfile
and `wasm-pack --target web --locked` already build the current core through
path dependencies. Browser and Node load the same emitted binary, not two
independently compiled engine versions.

## CI and release boundary

`npm-ci.yml` runs on **every push** and pull request, without secrets or publish
permissions. It builds, packs, executes the Node and real browser consumers,
checks Vite development/production, esbuild and Next production in Chromium,
checks both TypeScript resolution modes, and proves exports failures then
restores and retests. Rust compilation requires sccache at job scope and fails
if unavailable. Dependencies and Actions are pinned; Rust and WASM use the
committed WASM lockfile.

`npm-release.yml` accepts **only tag pushes** matching `v*`, remains disabled,
and checks the exact stable tag against the workspace version (e.g. `v0.1.0`).
It rebuilds and tests before uploading the exact tarball. A separate clean,
GitHub-hosted publish job downloads it and uses npm 11.16.0, `id-token: write`,
the `npm` environment and provenance. No npm token, secret, login, token
fallback configuration, manual dispatch, branch publish or npm lifecycle
script is configured. The publish job does not check out or build source.

Lint with:

```sh
actionlint -ignore 'constant expression "false"' .github/workflows/npm-ci.yml .github/workflows/npm-release.yml
```

The ignored diagnostic is the intentional hard disable; remove that exception
when enabling the workflow. Local validation is not a successful OIDC publish.

## Sami's npmjs.com setup and the first-publish blocker

1. Sign into npmjs.com. Profile picture → **Add an Organization** → Name
   **zega** → **Unlimited public packages** (free) → **Create** → optionally
   invite members → **Continue**.
2. The bare package **zega** must be owned by a **user account**. Creating the
   organization creates the `@zega` scope; it neither creates nor reserves the
   unscoped package. Do not rename this package to `@zega/zega`.
3. **Blocked before first publication:** npm's documented trusted-publisher
   setup starts in an existing package's settings. npm's September 17, 2026
   roadmap still lists automated first-package creation as future work.
   There is no verified npmjs.com click sequence to establish trust for this
   nonexistent bare package. Sami must resolve this with npm support or a
   supported token-free bootstrap. Keep this workflow disabled meanwhile.
   Do not add a token, log in through the CLI, publish a placeholder, or silently
   switch to a manual first publication; those would change the stated policy.
4. Once **zega** exists under Sami's control: npmjs.com → profile → **Packages**
   → **zega** → **Settings** → **Trusted publishing / Trusted Publisher** →
   **Add trusted publisher** → **GitHub Actions**. Enter:

   | Field | Exact value |
   | --- | --- |
   | Organization or user | `zegadb` (the GitHub organization, not npm org) |
   | Repository | `zega` |
   | Workflow filename | `npm-release.yml` (no path) |
   | Environment name | `npm` |
   | Allowed actions | Enable direct `npm publish` |

   Save the connection and complete any 2FA prompt. Current new connections
   default to staging permission; direct publish must be explicitly allowed.
5. Package **Settings** → **Publishing access** → **Require two-factor
   authentication and disallow tokens** → **Update Package Settings**.
6. On GitHub, Sami creates/configures the `npm` environment with himself as a
   required reviewer and release-tag protection. After resolving bootstrap and
   reviewing CI, Ava can remove the literal false guard in a reviewed change.
   Sami then chooses the workspace version and pushes the matching tag. Do not
   reuse an already-published version. This task creates or pushes no tags.

The registry configuration cannot be proven by saving a form: the actual
OIDC exchange happens on publication. No claim of a verified release is made.

Sources checked September 22, 2026:

- [npm organization creation](https://docs.npmjs.com/creating-an-organization/)
- [Unscoped packages are managed by user accounts](https://docs.npmjs.com/package-scope-access-level-and-visibility/)
- [npm trusted publishing requirements and settings](https://docs.npmjs.com/trusted-publishers/)
- [npm PM roadmap: first-package creation is still planned](https://github.com/orgs/community/discussions/208130)
- [Vite asset handling](https://vite.dev/guide/assets)
- [esbuild file loader](https://esbuild.github.io/content-types/#file)
