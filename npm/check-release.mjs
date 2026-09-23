import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { parse } from 'smol-toml';

const workspace = parse(await readFile(new URL('../Cargo.toml', import.meta.url), 'utf8')).workspace.package;
const pkg = JSON.parse(await readFile(new URL('../dist/package.json', import.meta.url), 'utf8'));
assert.equal(pkg.name, 'zega');
assert.equal(pkg.version, workspace.version);
assert.match(pkg.version, /^\d+\.\d+\.\d+$/, 'This workflow publishes stable versions only');
assert.equal(process.env.GITHUB_EVENT_NAME, 'push', 'Only tag pushes may publish');
assert.equal(process.env.GITHUB_REF, `refs/tags/v${workspace.version}`, 'Tag must match workspace.package.version');
console.log(`Release gate: ${process.env.GITHUB_REF} matches zega@${pkg.version}`);
