// The generated package is a reviewed artifact. Never derive its expected hash
// from the rebuild being checked, or a feature leak would approve itself.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
const root = fileURLToPath(new URL('../..', import.meta.url));
const browser = resolve(root, 'browser');
const mode = process.argv[2] ?? 'all';
assert(['all', 'dependencies', 'artifact'].includes(mode), 'usage: node browser/scripts/check-wasm.mjs [dependencies | artifact [wasm-file]]');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const expected = JSON.parse(readFileSync(resolve(browser, 'wasm-source.json')));
const wasm = resolve(browser, 'pkg/zega_wasm_bg.wasm');
function verify(path) {
  const actual = hash(readFileSync(path));
  assert.equal(actual, expected.sha256['zega_wasm_bg.wasm'], `WASM differs from the committed browser/pkg hash: ${path}`);
  console.log(`WASM SHA-256 ${actual}; ${readFileSync(path).length} bytes`);
}
if (mode !== 'artifact') {
  for (const target of [[], ['--target', 'wasm32-unknown-unknown']]) {
    const tree = execFileSync('cargo', ['tree', '--locked', '--manifest-path', 'zega-wasm/Cargo.toml', '-p', 'zega-wasm', '-e', 'normal', '--prefix', 'none', ...target], { cwd: root, encoding: 'utf8' });
    assert(!/^(worker(?:-[\w-]+)?|workerd|cloudflare(?:[-_][\w-]+)?) v/m.test(tree), `Cloudflare dependency leaked into zega-wasm:\n${tree}`);
    console.log(`cargo tree -p zega-wasm -e normal (${target.at(-1) ?? 'native'}): no Cloudflare crates`);
  }
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--manifest-path', 'zega-wasm/Cargo.toml', '--format-version', '1', '--filter-platform', 'wasm32-unknown-unknown'], { cwd: root, encoding: 'utf8' }));
  const engine = metadata.packages.find(p => p.name === 'zega');
  assert(engine, 'zega dependency must be present');
  assert(!metadata.resolve.nodes.find(n => n.id === engine.id).features.includes('durable-log'), 'zega-wasm must not enable zega/durable-log');
  assert(!engine.dependencies.some(d => /^(worker(?:-[\w-]+)?|wasm-bindgen-futures|cloudflare(?:[-_][\w-]+)?)$/.test(d.name)), 'zega must not depend on Cloudflare or wasm-bindgen-futures');
  console.log('zega/durable-log is disabled; engine has no Cloudflare dependencies');
}
if (mode === 'all') {
  verify(wasm);
  execFileSync(process.execPath, [resolve(browser, 'scripts/rebuild-wasm.mjs'), root], { cwd: root, stdio: 'inherit' });
  verify(wasm);
} else if (mode === 'artifact') {
  verify(process.argv[3] ? resolve(process.argv[3]) : wasm);
}
