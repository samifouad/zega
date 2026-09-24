// Builds the Rust Durable Object with worker-build 0.8.6 into ./build.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, chmodSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
const here = fileURLToPath(new URL(".", import.meta.url));
const root = resolve(here, "../../..");
const local = resolve(root, ".tmp/tools/bin/worker-build");
const executable = process.env.WORKER_BUILD ?? (existsSync(local) ? local : "worker-build");
const tmp = resolve(root, ".tmp");
mkdirSync(tmp, { recursive: true });
chmodSync(tmp, 0o700);
const result = spawnSync(executable, ["--release", "--out-dir", "build", "--", "--locked"], {
  cwd: here,
  stdio: "inherit",
  env: { ...process.env, CARGO_TARGET_DIR: resolve(root, ".target"), TMPDIR: tmp },
});
if (result.error) console.error(`${result.error.message}; install worker-build 0.8.6 (see README)`);
process.exit(result.status ?? 1);
