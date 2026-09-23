import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

process.chdir(resolve(import.meta.dirname, '..'));
await mkdir('artifacts', { recursive: true });
const [pack] = JSON.parse(execFileSync('npm', ['pack', './dist', '--json', '--ignore-scripts', '--pack-destination', 'artifacts'], { encoding: 'utf8' }));
assert.deepEqual(pack.files.map(file => file.path).sort(), [
  'LICENSE', 'README.md', 'browser.js', 'index.d.ts', 'node.js', 'package.json',
  'wasm/zega_wasm.js', 'wasm/zega_wasm.d.ts', 'wasm/zega_wasm_bg.wasm', 'wasm/zega_wasm_bg.wasm.d.ts',
].sort(), 'Unexpected npm tarball contents');
await writeFile('artifacts/pack.json', JSON.stringify(pack, null, 2) + '\n');
console.log(`${pack.filename}: ${pack.size} bytes packed, ${pack.unpackedSize} bytes unpacked`);
for (const file of pack.files) console.log(`${file.path} (${file.size} bytes)`);
