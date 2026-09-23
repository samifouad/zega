import { cp, mkdir, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import './check.mjs';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
await rm('dist', { recursive: true, force: true });
await mkdir('dist');
// Explicit deploy inputs keep repository metadata, tooling and local files out.
for (const path of ['index.html', 'style.css', 'repl.js', 'editor.js', 'graph.js', 'csv.js', 'pkg', 'vendor']) {
  await cp(path, `dist/${path}`, { recursive: true });
}
await cp('../LICENSE', 'dist/LICENSE');
console.log('Built static explorer in dist/ using the vendored wasm (no engine checkout required).');
