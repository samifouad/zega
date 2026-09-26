# Zega Cloud's Fly Machines image

`registry.fly.io/zega-g:latest` is the image Zega Cloud's control plane
(zegadb/cloud, `FlyMachines.create` in `src/machines/fly.ts`) runs one Fly
Machine of per Pro graph. The Dockerfile lives here, in zegadb/zega, because
the image is just the `zega` CLI (`zega-cli`, `[[bin]] name = "zega"`) with a
non-root entrypoint — no code that belongs to the control plane repo.

Build inputs, both fixed by the control plane's machine config
(`src/machines/fly.ts`) and not this Dockerfile's to choose:

- **Command**: Fly's `init.cmd` supplies the container's argv directly
  (replacing CMD, not ENTRYPOINT) —
  `zega start --host :: --port 8080 --data /data/zega --token-file
  /etc/zega/token`. `--token-file` is `zega-cli`'s existing flag
  (`zega-cli/src/main.rs`, `Command::Start`); nothing to add there.
- **Port**: `services[0].internal_port` is `8080`; the container must listen
  on `0.0.0.0`/`::` on that port, which `--host ::` on Linux's dual-stack
  default already satisfies.
- **Data**: the Fly volume is mounted at `/data`; `zega` is told to use
  `/data/zega` so the mount root itself (owned by Fly, not this image) is
  never written to directly.
- **Token**: Fly writes the per-graph token to `/etc/zega/token` as a file
  (`files: [{ guest_path: "/etc/zega/token", raw_value: base64(token) }]`)
  before the machine starts; `zega start --token-file` reads it. There is no
  environment variable in this path — `zega`'s rule is that product behaviour
  is configured by explicit flags/config, never env vars.
- **Health / auth**: `GET /health` (zega-server's existing route) requires
  the same bearer token as every other route
  (`zega-server/src/handlers.rs::authorized`) — 200 with the right
  `Authorization: Bearer <token>`, 401 with the wrong one or none. There is no
  separate unauthenticated health path; the control plane's `services[]`
  config does not (yet) declare a Fly-level `checks` entry, so this endpoint
  is for the router/operator, not Fly's own health checker.

## Non-root and the Fly volume

The runtime stage is `debian:bookworm-slim`, not distroless: a **fresh** Fly
volume is mounted at `/data` owned `root:root 0755`
(community-documented: <https://community.fly.io/t/fly-volumes-getting-permission-denied/1773>),
so a non-root process cannot `mkdir /data/zega` on first boot. `entrypoint.sh`
starts as root (the container's default), `chown`s `/data` to the `zega`
user once (non-recursive — `zega` then owns everything it creates under
`/data`, so this stays O(1), not O(data size), on every restart), and then
uses `setpriv` (already in `bookworm-slim`, no extra package) to drop to
`zega` (uid 10001) before `exec`ing the real `zega` binary. The server itself
always runs as non-root; only the one-line ownership fixup runs as root.

## Build

```sh
cd <zegadb/zega checkout>   # repo root: the Dockerfile's build context
umask 022
export CARGO_TARGET_DIR="$PWD/.target" TMPDIR="$PWD/.tmp"
mkdir -p "$TMPDIR" && chmod 700 "$TMPDIR"

docker build --platform linux/amd64 \
  -f docker/zega-g/Dockerfile \
  -t registry.fly.io/zega-g:latest .
```

`--locked --release -p zega-cli` inside the build stage matches the repo's
own release build (`.github/workflows/release.yml`, "Build CLI with embedded
explorer"); the Rust toolchain is pinned to `1.96.0`, the same version
`dtolnay/rust-toolchain` pins in that workflow.

## Push (Ava only — this is a credentialed step)

```sh
docker login registry.fly.io -u x --password-stdin   # Fly deploy token on stdin
docker push registry.fly.io/zega-g:latest
```

The Fly deploy token is a secret Sami provides; never paste it in a file,
chat or log (AGENTS.md §0.3). Building locally needs no credential — only the
push does.

## Verify locally (no Fly account needed)

Named Docker volumes, not host bind mounts: this is also closer to Fly's own
model (a Fly volume and a Fly `files` write are not host paths either), and
it sidesteps a Docker-Desktop/Colima quirk where a bind-mounted host
directory that the VM backend doesn't share comes up silently empty inside
the container.

```sh
cd <zegadb/zega checkout>   # repo root

docker volume create zega-g-verify-data
docker volume create zega-g-verify-secrets

# Seed the token file into the secrets volume (root:root; entrypoint.sh
# chowns it to `zega` at container start, same as it does for /data).
docker run --rm --entrypoint sh -v zega-g-verify-secrets:/etc/zega \
  registry.fly.io/zega-g:latest \
  -c 'printf %s test-token > /etc/zega/token && chmod 600 /etc/zega/token'

docker run -d --name zega-g-verify \
  -p 18080:8080 \
  -v zega-g-verify-data:/data \
  -v zega-g-verify-secrets:/etc/zega \
  registry.fly.io/zega-g:latest \
  start --host :: --port 8080 --data /data/zega --token-file /etc/zega/token

curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18080/health \
  -H 'authorization: Bearer test-token'      # expect 200
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18080/health
                                              # expect 401 (no token)
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:18080/zql \
  -H 'authorization: Bearer test-token' -H 'content-type: application/json' \
  -d '{"schema":"type Person { name: String }","query":"{ Person { name } }"}'
                                              # expect 200

docker logs zega-g-verify
docker rm -f zega-g-verify
docker volume rm zega-g-verify-data zega-g-verify-secrets
```

The command line matches `FlyMachines.config()`'s `init.cmd` exactly (values
only, not the flags), so this is the same argv Fly hands the container in
production.

## CI

`.github/workflows/docker-image.yml` builds this image (no push) on any PR
or push to `main` that touches `docker/zega-g/**` or the crates it packages
(`zega`, `zega-server`, `zega-cli`, `Cargo.lock`). It has no registry
credentials and is a build-only smoke check — it does not run or curl the
image; that stays a manual/Ava step until there is a reason to make it part
of CI (e.g. `docker run --rm` + curl in the same job, once caching makes the
extra minute cheap).
