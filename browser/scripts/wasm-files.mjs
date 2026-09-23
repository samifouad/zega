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
