import { spawn } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
export const root = fileURLToPath(new URL('../..', import.meta.url));
export async function localServer({ port = 8799, name = 'test', snapshotBytes = 8388608, controls = true } = {}) {
  const dir = resolve(root, '.tmp', name);
  await mkdir(dir, { recursive: true });
  // A copied local config avoids recompilation on every process restart. Never
  // inherit Cloudflare credentials or use the user's Wrangler auth directory.
  let config = await readFile(resolve(root, 'zega-cloud/wrangler.toml'), 'utf8');
  config = config.replace('main = "entry.mjs"', `main = ${JSON.stringify(resolve(root, 'zega-cloud/entry.mjs'))}`)
    .replace('[build]\ncommand = "node build.mjs"\n', '')
    .replace('SPIKE_CONTROLS = "false"', `SPIKE_CONTROLS = "${controls}"`)
    .replace('SNAPSHOT_BYTES = "8388608"', `SNAPSHOT_BYTES = "${snapshotBytes}"`);
  await writeFile(resolve(dir, 'wrangler.toml'), config);
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(CLOUDFLARE_|CF_|WRANGLER_)/.test(key)));
  Object.assign(env, { XDG_CONFIG_HOME: resolve(dir, 'config'), WRANGLER_SEND_METRICS: 'false', CI: 'true', TMPDIR: resolve(root, '.tmp') });
  const child = spawn(process.execPath, [resolve(root, 'zega-cloud/node_modules/wrangler/bin/wrangler.js'),
    'dev', '--local', '--ip', '127.0.0.1', '--port', String(port), '--inspector-port', '0',
    '--config', resolve(dir, 'wrangler.toml'), '--persist-to', resolve(dir, 'state'),
    '--show-interactive-dev-session=false', '--log-level', 'error'], {
    cwd: dir, env, detached: true, stdio: ['ignore', 'pipe', 'pipe'],
  });
  const log = createWriteStream(resolve(dir, 'wrangler.log'), { flags: 'a' });
  child.stdout.pipe(log); child.stderr.pipe(log);
  const endpoint = `http://127.0.0.1:${port}`;
  async function stop(signal = 'SIGTERM') {
    if (child.exitCode !== null) return;
    try { process.kill(-child.pid, signal); } catch (e) { if (e.code !== 'ESRCH') throw e; }
    await Promise.race([new Promise(r => child.once('exit', r)), delay(5000)]);
    if (child.exitCode === null) { try { process.kill(-child.pid, 'SIGKILL'); } catch {} }
    log.end();
  }
  for (let attempt = 0; attempt < 150; attempt++) {
    if (child.exitCode !== null) throw new Error(`wrangler exited ${child.exitCode}: ${await readFile(resolve(dir, 'wrangler.log'), 'utf8')}`);
    try {
      const response = await fetch(`${endpoint}/ready`, { signal: AbortSignal.timeout(500) });
      if (response.status === 404) return { endpoint, stop, child, dir };
    } catch {}
    await delay(200);
  }
  await stop();
  throw new Error(`local workerd startup timeout; see ${dir}/wrangler.log`);
}
