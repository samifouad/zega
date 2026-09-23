import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { parse } from 'smol-toml';

const root = resolve(import.meta.dirname, '..');
process.chdir(root);
const workspace = parse(await readFile('Cargo.toml', 'utf8')).workspace.package;
const wasm = parse(await readFile('zega-wasm/Cargo.toml', 'utf8')).package;
if (wasm.version !== workspace.version) {
  throw new Error('Set zega-wasm/Cargo.toml version to workspace.package.version and refresh its Cargo.lock before building.');
}
if (wasm.repository !== workspace.repository) throw new Error('WASM repository metadata must match the workspace.');

await rm('dist', { recursive: true, force: true });
await mkdir('dist', { recursive: true });
execFileSync('wasm-pack', ['build', 'zega-wasm', '--target', 'web', '--out-dir', '../dist/wasm', '--release', '--locked'], {
  stdio: 'inherit',
  env: { ...process.env, CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? resolve('.target') },
});
// Keep only the runtime and both wasm-bindgen declarations, not its second manifest.
for (const file of ['package.json', '.gitignore', 'README.md', 'LICENSE']) {
  await rm(`dist/wasm/${file}`, { force: true });
}
for (const file of ['browser.js', 'node.js', 'index.d.ts']) await cp(`npm/src/${file}`, `dist/${file}`);
await cp('npm/README.md', 'dist/README.md');
await cp('LICENSE', 'dist/LICENSE');
await writeFile('dist/package.json', JSON.stringify({
  name: 'zegadb',
  version: workspace.version,
  description: 'An in-memory graph database and KV engine with ZQL v2, for browsers and Node.js',
  type: 'module',
  license: workspace.license,
  repository: { type: 'git', url: workspace.repository },
  homepage: 'https://github.com/zegadb/zega#readme',
  bugs: 'https://github.com/zegadb/zega/issues',
  engines: { node: '>=22.14.0' },
  main: './node.js',
  module: './browser.js',
  types: './index.d.ts',
  exports: {
    '.': { types: './index.d.ts', browser: './browser.js', node: './node.js', default: './browser.js' },
    './wasm': { types: './wasm/zega_wasm.d.ts', default: './wasm/zega_wasm.js' },
    './zega_wasm_bg.wasm': './wasm/zega_wasm_bg.wasm',
  },
  // wasm-bindgen glue initializes module state; keep it during tree shaking.
  sideEffects: ['./wasm/zega_wasm.js'],
  files: ['browser.js', 'node.js', 'index.d.ts', 'wasm/*.js', 'wasm/*.wasm', 'wasm/*.d.ts', 'README.md', 'LICENSE'],
  publishConfig: { access: 'public', registry: 'https://registry.npmjs.org/' },
}, null, 2) + '\n');
console.log(`Built zegadb@${workspace.version} in dist/`);
