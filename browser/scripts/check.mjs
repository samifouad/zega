import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { hashFiles, hostPaths } from './wasm-files.mjs';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
const source = JSON.parse(await readFile('wasm-source.json', 'utf8'));
assert.match(source.commit, /^[0-9a-f]{40}$/);
assert.deepEqual(await hashFiles('pkg'), source.sha256, 'pkg/ changed without a matching wasm-source.json; rebuild and commit them together');
const wasm = await readFile('pkg/zega_wasm_bg.wasm');
await WebAssembly.compile(wasm);
const leaked = hostPaths(wasm);
assert.deepEqual(leaked.slice(0, 10), [], `pkg/zega_wasm_bg.wasm embeds ${leaked.length} build-machine paths; rebuild with npm run wasm:rebuild (#45)`);

let count = 0;
async function checkDirectory(dir, recursive = false) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (recursive) await checkDirectory(`${dir}/${entry.name}`, true);
      continue;
    }
    if (!/\.(mjs|js)$/.test(entry.name)) continue;
    execFileSync(process.execPath, ['--check', `${dir}/${entry.name}`], { stdio: 'inherit' });
    count++;
  }
}
for (const dir of ['.', 'pkg', 'vendor', 'scripts', 'tests', 'cli-tests']) await checkDirectory(dir, dir === 'vendor');
console.log(`PASS: ${count} JavaScript syntax checks; wasm compiles and embeds no build-machine paths; all pkg/ SHA-256 hashes match engine ${source.commit}`);
