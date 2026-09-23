import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { hashFiles } from './wasm-files.mjs';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
const source = JSON.parse(await readFile('wasm-source.json', 'utf8'));
assert.match(source.commit, /^[0-9a-f]{40}$/);
assert.deepEqual(await hashFiles('pkg'), source.sha256, 'pkg/ changed without a matching wasm-source.json; rebuild and commit them together');
await WebAssembly.compile(await readFile('pkg/zega_wasm_bg.wasm'));

let count = 0;
for (const dir of ['.', 'pkg', 'vendor', 'scripts', 'tests', 'cli-tests']) {
  for (const file of await readdir(dir)) {
    if (!/\.(mjs|js)$/.test(file)) continue;
    execFileSync(process.execPath, ['--check', `${dir}/${file}`], { stdio: 'inherit' });
    count++;
  }
}
console.log(`PASS: ${count} JavaScript syntax checks; wasm compiles; all pkg/ SHA-256 hashes match engine ${source.commit}`);
