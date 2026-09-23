import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { chromium } from 'playwright';
import { build as viteBuild, createServer, preview } from 'vite';
import { build as esbuild } from 'esbuild';

const root = resolve(import.meta.dirname, '..');
process.chdir(root);
const work = resolve('.tmp/consumers');
const manifestPath = resolve('dist/package.json');

function run(command, args, cwd = root) {
  return execFileSync(command, args, { cwd, stdio: 'inherit' });
}

async function installConsumers() {
  const pack = JSON.parse(await readFile('artifacts/pack.json', 'utf8'));
  await rm(work, { recursive: true, force: true });
  for (const consumer of ['node', 'browser']) {
    const cwd = resolve(work, consumer);
    await cp(`npm/consumers/${consumer}`, cwd, { recursive: true });
    run('npm', ['install', resolve('artifacts', pack.filename), '--ignore-scripts', '--no-save', '--no-package-lock', '--no-audit', '--no-fund'], cwd);
  }
}

async function checkBrowser(outDir, label, dev = false) {
  const server = dev ? await createServer({
    root: resolve(work, 'browser'), server: { host: '127.0.0.1', port: 0 },
  }) : await preview({
    configFile: false, root: resolve(work, 'browser'), base: '/consumer/',
    build: { outDir }, preview: { host: '127.0.0.1', port: 0 },
  });
  let browser;
  try {
    if (dev) await server.listen();
    browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
    const page = await browser.newPage();
    const errors = [];
    const assets = [];
    page.on('pageerror', error => errors.push(String(error)));
    page.on('response', response => { if (new URL(response.url()).pathname.endsWith('.wasm')) assets.push(response); });
    await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/consumer/`);
    await page.waitForFunction(() => document.querySelector('#result').textContent !== 'Loading');
    const result = await page.locator('#result').textContent();
    assert.deepEqual(errors, []);
    assert.equal(result, '{"name":"Ada"}');
    assert.ok(assets.length > 0, 'Production browser must fetch a real WASM asset');
    for (const asset of assets) {
      assert.equal(asset.status(), 200);
      assert.match(asset.headers()['content-type'], /application\/wasm/);
    }
    console.log(`${label}: ${result} (WASM HTTP 200, application/wasm)`);
  } finally {
    await browser?.close();
    if (dev) await server.close();
    else await new Promise((done, reject) => server.httpServer.close(error => error ? reject(error) : done()));
  }
}

async function checkConsumers() {
  await installConsumers();
  run('node', ['index.mjs'], resolve(work, 'node'));
  const tsc = resolve('node_modules/typescript/bin/tsc');
  const flags = ['--noEmit', '--strict', '--target', 'es2022', '--lib', 'es2022,dom,esnext.disposable'];
  run('node', [tsc, ...flags, '--module', 'NodeNext', '--moduleResolution', 'NodeNext', 'types.mts'], resolve(work, 'node'));
  await cp(resolve(work, 'node/types.mts'), resolve(work, 'browser/types.mts'));
  run('node', [tsc, ...flags, '--module', 'ESNext', '--moduleResolution', 'Bundler', 'types.mts'], resolve(work, 'browser'));

  await viteBuild({ root: resolve(work, 'browser') });
  await checkBrowser(resolve(work, 'browser/dist'), 'browser/vite');
  await checkBrowser(undefined, 'browser/vite-dev', true);

  // esbuild requires an explicit asset import/file loader; its new URL handling
  // differs from Vite/Webpack. Exercise the documented override against the tarball.
  const outDir = resolve(work, 'browser/dist-esbuild');
  await mkdir(outDir, { recursive: true });
  await writeFile(resolve(work, 'browser/esbuild-main.js'), `import wasmURL from 'zega/zega_wasm_bg.wasm';\nimport { run } from './query.js';\nawait run({ wasm: wasmURL });\n`);
  await esbuild({
    absWorkingDir: resolve(work, 'browser'), entryPoints: ['esbuild-main.js'],
    bundle: true, minify: true, format: 'esm', platform: 'browser', target: 'es2022',
    loader: { '.wasm': 'file' }, outdir: outDir, publicPath: '/consumer/',
  });
  await writeFile(resolve(outDir, 'index.html'), '<!doctype html><pre id="result">Loading</pre><script type="module" src="/consumer/esbuild-main.js"></script>');
  await checkBrowser(outDir, 'browser/esbuild');
}

if (process.argv.includes('--break-exports')) {
  const original = await readFile(manifestPath, 'utf8');
  try {
    const broken = JSON.parse(original);
    broken.exports['.'] = './missing-entry.js';
    await writeFile(manifestPath, JSON.stringify(broken, null, 2) + '\n');
    run('node', ['npm/pack.mjs']);
    await installConsumers();
    const node = spawnSync('node', ['index.mjs'], { cwd: resolve(work, 'node'), encoding: 'utf8' });
    assert.notEqual(node.status, 0, 'Broken exports unexpectedly passed Node');
    assert.match(node.stderr, /ERR_MODULE_NOT_FOUND/);
    console.log(`broken exports/node: exit ${node.status}; ${node.stderr.split('\n').find(line => line.includes('Error ['))}`);
    await assert.rejects(viteBuild({ root: resolve(work, 'browser'), logLevel: 'silent' }), /[Ff]ailed to resolve (entry for package|import) "zega"/);
    console.log('broken exports/browser: Vite failed to resolve import "zega"');
  } finally {
    await writeFile(manifestPath, original);
    run('node', ['npm/pack.mjs']);
    console.log('Restored original exports and repacked.');
  }
}
await checkConsumers();
