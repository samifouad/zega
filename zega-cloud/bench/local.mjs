// Reproduce all sizes with separate local processes/isolate heaps, no account.
import { spawn } from 'node:child_process';
import { appendFile, mkdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { root, localServer } from '../test/local-server.mjs';
const output = resolve(root, '.tmp/evidence/bench.jsonl');
await mkdir(resolve(root, '.tmp/evidence'), { recursive: true });
await rm(output, { force: true });
const sizes = process.argv.slice(2).map(Number);
for (const mb of sizes.length ? sizes : [10, 50, 100]) {
  if (![10, 50, 100].includes(mb)) throw new Error('local sizes: 10 50 100');
  await rm(resolve(root, `.tmp/bench-${mb}`), { recursive: true, force: true });
  const server = await localServer({ name: `bench-${mb}`, port: 8801 });
  try {
    for (const script of ['load', 'cold', 'read', 'write']) {
      const args = [resolve(root, `zega-cloud/bench/${script}.mjs`), server.endpoint, `bench-${mb}`];
      if (script === 'load') args.push(String(mb));
      const child = spawn(process.execPath, args, { stdio: ['ignore', 'pipe', 'inherit'] });
      let outputLine = '';
      child.stdout.on('data', chunk => { outputLine += chunk; process.stdout.write(chunk); });
      const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
      await appendFile(output, outputLine);
      // A failed larger load is a measured limit, never a passing latency run
      // on the silently smaller surviving prefix.
      if (code !== 0) { process.exitCode = 1; break; }
    }
  } finally { await server.stop(); }
}
