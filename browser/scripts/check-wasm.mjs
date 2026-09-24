// Regular zega-wasm must never pick up the Cloud host: no Cloudflare crate in its
// dependency graph and no `zega/durable-log` through feature unification.
// These checks read Cargo's resolved graph, so they are deterministic and need no
// build. A rebuilt-binary hash is not a gate: the binary depends on the checkout
// path (Cargo's package ids feed symbol hashes), so it differs across machines.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../..', import.meta.url));
const cloudflare = /^(worker(?:-[\w-]+)?|workerd|cloudflare(?:[-_][\w-]+)?) v/m;
for (const target of [[], ['--target', 'wasm32-unknown-unknown']]) {
  const tree = execFileSync('cargo', ['tree', '--locked', '--manifest-path', 'zega-wasm/Cargo.toml', '-p', 'zega-wasm', '-e', 'normal', '--prefix', 'none', ...target], { cwd: root, encoding: 'utf8' });
  assert(!cloudflare.test(tree), `Cloudflare dependency leaked into zega-wasm:\n${tree}`);
  console.log(`cargo tree -p zega-wasm -e normal (${target.at(-1) ?? 'native'}): no Cloudflare crates`);
}
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--manifest-path', 'zega-wasm/Cargo.toml', '--format-version', '1', '--filter-platform', 'wasm32-unknown-unknown'], { cwd: root, encoding: 'utf8' }));
const engine = metadata.packages.find(p => p.name === 'zega');
assert(engine, 'zega dependency must be present');
assert(!metadata.resolve.nodes.find(n => n.id === engine.id).features.includes('durable-log'), 'zega-wasm must not enable zega/durable-log');
assert(!engine.dependencies.some(d => /^(worker(?:-[\w-]+)?|wasm-bindgen-futures|cloudflare(?:[-_][\w-]+)?)$/.test(d.name)), 'zega must not depend on Cloudflare or wasm-bindgen-futures');
console.log('zega/durable-log is disabled; engine has no Cloudflare dependencies');
