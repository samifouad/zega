import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import './check.mjs';

// `--remote` builds explorer2 into dist-remote/: the same bundle, with the
// database behind a remote zega server (remote/backend.js) instead of wasm.
// `--remote-base=URL` points it at another server (default: the Fly app
// zega-explorer2-api, zega-cloud/explorer2-api/fly.toml).
const REMOTE_BASE = 'https://zega-explorer2-api.fly.dev';
const remote = process.argv.includes('--remote');
const base = process.argv.find((arg) => arg.startsWith('--remote-base='))?.slice('--remote-base='.length) ?? REMOTE_BASE;
const out = remote ? 'dist-remote' : 'dist';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
await rm(out, { recursive: true, force: true });
await mkdir(out);
// Explicit deploy inputs keep repository metadata, tooling and local files out.
for (const path of JSON.parse(await readFile('assets.json', 'utf8'))) {
  await cp(path, `${out}/${path}`, { recursive: true });
}
await cp('../LICENSE', `${out}/LICENSE`);
if (remote) {
  const url = new URL(base);
  if (url.protocol !== 'https:' && !['127.0.0.1', 'localhost'].includes(url.hostname)) {
    throw new Error(`--remote-base must be https (or local): ${base}`);
  }
  // The remote backend reuses the native protocol class from backend.js as is.
  await writeFile(`${out}/native.js`, `${await readFile('backend.js', 'utf8')}\nexport { NativeDatabase };\n`);
  await cp('remote/backend.js', `${out}/backend.js`);
  await cp('remote/remote.css', `${out}/remote.css`);
  // No token here: the page asks for it and keeps it in localStorage.
  await writeFile(`${out}/explorer-config.json`, `${JSON.stringify({ backend: 'remote', base: url.origin })}\n`);
  console.log(`Built remote explorer in ${out}/ against ${url.origin}.`);
} else {
  console.log('Built static explorer in dist/ using the vendored wasm (no engine checkout required).');
}
