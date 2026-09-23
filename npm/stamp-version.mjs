import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';

const path = new URL('../dist/package.json', import.meta.url);
const pkg = JSON.parse(await readFile(path, 'utf8'));
const version = process.argv[2];
assert.equal(pkg.name, 'zegadb');
assert.ok(version.startsWith(`${pkg.version}-canary.`));
assert.match(version, /^\d+\.\d+\.\d+-canary\.[0-9a-f]{7}$/);
pkg.version = version;
await writeFile(path, JSON.stringify(pkg, null, 2) + '\n');
