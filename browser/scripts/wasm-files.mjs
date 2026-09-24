import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function hashFiles(directory, prefix = '') {
  const hashes = {};
  for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
    const relative = prefix + entry.name;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) Object.assign(hashes, await hashFiles(path, relative + '/'));
    else if (entry.isFile()) hashes[relative] = createHash('sha256').update(await readFile(path)).digest('hex');
    else throw new Error(`Unexpected non-file in wasm package: ${path}`);
  }
  return hashes;
}

/**
 * rustc flags that rewrite build-machine paths in panic locations (#45): the
 * engine checkout, the cargo target dir, CARGO_HOME (registry sources) and the
 * rustc sysroot. rustc applies the last matching remap, so the target dir,
 * usually inside the checkout, comes after it.
 */
export function remapFlags({ engine, targetDir, cargoHome, sysroot }) {
  return [[engine, '/zega'], [targetDir, '/target'], [cargoHome, '/cargo'], [sysroot, '/rust']]
    .flatMap(([from, to]) => ['--remap-path-prefix', `${from}=${to}`]);
}

/** CARGO_ENCODED_RUSTFLAGS: the caller's own flags, then `extra`. Separated by 0x1f, so paths may contain spaces. */
export function encodedRustflags(env, extra) {
  const existing = env.CARGO_ENCODED_RUSTFLAGS
    ? env.CARGO_ENCODED_RUSTFLAGS.split('\x1f')
    : (env.RUSTFLAGS ?? '').split(/\s+/).filter(Boolean);
  return [...existing, ...extra].join('\x1f');
}

/**
 * Absolute build-machine paths found in a binary: home, volume and temp
 * directories, and any of the given local paths. Each is reported once.
 */
export function hostPaths(bytes, localPaths = []) {
  const text = Buffer.from(bytes).toString('latin1');
  const found = new Set(text.match(/\/(?:Users|Volumes|home|root|private|tmp)\/[\x21-\x7e]+|[A-Za-z]:\\[\x21-\x7e]+/g) ?? []);
  for (const local of localPaths) if (local && text.includes(local)) found.add(local);
  return [...found];
}
