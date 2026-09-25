// Swap time and heap of the graph-components canvas (APS 21) over 200 swaps,
// headless. Starts the explorer's dev server itself.
//   node scripts/bench-components.mjs [--gpu]
// --gpu runs full Chromium on ANGLE Metal (the machine's GPU) instead of the
// headless shell's SwiftShader, which rasterises every 1440x1000 frame on the
// CPU (~30 ms a frame) and so measures the software renderer, not the canvas.
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { collectedHeap, openComponents, renderer, summary, swaps } from '../tests/components-helpers.js';

const args = new Set(process.argv.slice(2));
const PORT = 8792;
process.chdir(fileURLToPath(new URL('..', import.meta.url)));
// Its own process group, so the finally below stops wrangler and workerd too, not only npm.
const server = spawn('npm', ['run', 'dev', '--', '--port', String(PORT)], { stdio: 'ignore', detached: true, env: { ...process.env, WRANGLER_SEND_METRICS: 'false' } });
const deadline = Date.now() + 90_000;
for (;;) {
  try { if ((await fetch(`http://127.0.0.1:${PORT}/`)).ok) break; } catch { /* not up yet */ }
  if (Date.now() > deadline) throw new Error('dev server did not start');
  await new Promise((resolve) => setTimeout(resolve, 500));
}
try {
  const browser = await chromium.launch({
    headless: true,
    ...(args.has('--gpu') ? { channel: 'chromium', args: ['--use-angle=metal', '--ignore-gpu-blocklist'] } : {}),
  });
  const page = await browser.newPage({ baseURL: `http://127.0.0.1:${PORT}`, viewport: { width: 1440, height: 1000 } });
  await openComponents(page, { history: 8 });
  const cdp = await page.context().newCDPSession(page);
  await swaps(page, 200); // warm
  const before = await collectedHeap(cdp);
  const { times } = await swaps(page, 200);
  const after = await collectedHeap(cdp);
  const { median, p95, max } = summary(times);
  console.log(`renderer: ${await renderer(page)}`);
  console.log(`200 swaps: median ${median.toFixed(1)} ms, p95 ${p95.toFixed(1)} ms, max ${max.toFixed(1)} ms`);
  console.log(`heap after forced GC: ${(before / 1e6).toFixed(2)} MB -> ${(after / 1e6).toFixed(2)} MB (${((after - before) / 1e3).toFixed(0)} KB)`);
  await browser.close();
} finally {
  process.kill(-server.pid);
}
