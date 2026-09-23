import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { cp, readFile, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { once } from 'node:events';
import { chromium } from 'playwright';

const root = resolve(import.meta.dirname, '..');
const cwd = resolve(root, '.tmp/next-consumer');
const pack = JSON.parse(await readFile(resolve(root, 'artifacts/pack.json'), 'utf8'));
await rm(cwd, { recursive: true, force: true });
await cp(resolve(root, 'npm/consumers/next'), cwd, { recursive: true });
execFileSync('npm', ['install', resolve(root, 'artifacts', pack.filename), '--ignore-scripts', '--no-save', '--no-package-lock', '--no-audit', '--no-fund'], { cwd, stdio: 'inherit' });
const next = resolve(root, 'node_modules/next/dist/bin/next');
const env = { ...process.env, NEXT_TELEMETRY_DISABLED: '1' };
execFileSync('node', [next, 'build'], { cwd, env, stdio: 'inherit' });
const server = spawn('node', [next, 'start', '--hostname', '127.0.0.1', '--port', '0'], { cwd, env, stdio: ['ignore', 'pipe', 'pipe'] });
const exited = once(server, 'exit');
let browser;
try {
  const url = await new Promise((done, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error(`Next startup timed out: ${output}`)), 30000);
    function read(data) {
      output += data;
      if (/Ready in/.test(output)) {
        clearTimeout(timer);
        done(output.match(/http:\/\/127\.0\.0\.1:\d+/)[0]);
      }
    }
    server.stdout.on('data', read);
    server.stderr.on('data', read);
    server.on('exit', code => { clearTimeout(timer); reject(new Error(`Next exited ${code}: ${output}`)); });
    server.on('error', reject);
  });
  const response = await fetch(`${url}/api/query`);
  assert.equal(response.status, 200);
  const serverResult = await response.json();
  assert.deepEqual(serverResult, { name: 'Ada' });
  console.log(`next/server: ${JSON.stringify(serverResult)}`);
  browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(String(error)));
  await page.goto(url);
  await page.waitForFunction(() => document.querySelector('#result')?.textContent !== 'Loading');
  const result = await page.locator('#result').textContent();
  assert.deepEqual(errors, []);
  assert.equal(result, '{"name":"Ada"}');
  console.log(`browser/next: ${result}`);
} finally {
  await browser?.close();
  server.kill();
  await exited;
}
