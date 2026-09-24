// Frame times of the globe's arcs (zega#74) with 0, 500 and 5,000 animated
// arcs, headless. Starts the explorer's dev server itself.
//   node scripts/bench-arcs.mjs [--gpu] [--uncapped]
// --gpu runs full Chromium on ANGLE Metal (the machine's GPU) instead of the
// headless shell's SwiftShader; --uncapped removes the compositor's frame-rate
// limit so the deltas show the real cost rather than vsync.
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { globe } from '../tests/globe-helpers.js';

const args = new Set(process.argv.slice(2));
const PORT = 8791;
const SCHEMA = `schema {
  type City { name: String at: Point }
  display { globe(@zoom: 1.6, @center: @point(30, -20)) { City } : Default }
}
mutation { City(name: "Calgary" && at: @point(51.05, -114.07)) { name } }`;

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
const server = spawn('npm', ['run', 'dev', '--', '--port', String(PORT)], { stdio: 'ignore', env: { ...process.env, WRANGLER_SEND_METRICS: 'false' } });
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
    ...(args.has('--uncapped') ? { args: [...(args.has('--gpu') ? ['--use-angle=metal', '--ignore-gpu-blocklist'] : []), '--disable-frame-rate-limit', '--disable-gpu-vsync'] } : {}),
  });
  const page = await browser.newPage({ baseURL: `http://127.0.0.1:${PORT}`, viewport: { width: 1440, height: 1000 } });
  await globe(page, SCHEMA);
  const renderer = await page.evaluate(() => {
    const gl = document.querySelector('#graph')._map.painter.context.gl;
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
  });
  console.log(`renderer: ${renderer}`);
  console.log(`viewport 1440x1000, ${args.has('--uncapped') ? 'frame-rate limit off' : 'vsync'}; ${240} frames per row after a 1 s warm-up`);
  console.log('arcs\tmean ms\tp50 ms\tp95 ms\tmax ms\tfps');
  for (const n of [0, 500, 5000]) {
    const stats = await page.evaluate(async (n) => {
      const layer = document.querySelector('#graph')._arcs;
      let seed = 42;
      const random = () => { seed = (seed * 1664525 + 1013904223) % 4294967296; return seed / 4294967296; };
      const place = () => [random() * 360 - 180, Math.asin(random() * 2 - 1) * 180 / Math.PI];
      layer.setArcs(Array.from({ length: n }, (_, i) => ({ from: place(), to: place(), rel: { id: i } })));
      layer.setSettings({ animate: true });
      await new Promise((resolve) => setTimeout(resolve, 1000));
      const deltas = await new Promise((resolve) => {
        const times = [];
        let last;
        const tick = (now) => {
          if (last !== undefined) times.push(now - last);
          last = now;
          if (times.length < 240) requestAnimationFrame(tick); else resolve(times);
        };
        requestAnimationFrame(tick);
      });
      const sorted = [...deltas].sort((a, b) => a - b);
      const mean = deltas.reduce((a, b) => a + b, 0) / deltas.length;
      return { mean, p50: sorted[Math.floor(sorted.length / 2)], p95: sorted[Math.floor(sorted.length * 0.95)], max: sorted[sorted.length - 1] };
    }, n);
    console.log(`${n}\t${stats.mean.toFixed(2)}\t${stats.p50.toFixed(2)}\t${stats.p95.toFixed(2)}\t${stats.max.toFixed(2)}\t${(1000 / stats.mean).toFixed(1)}`);
  }
  await browser.close();
} finally {
  server.kill();
}
