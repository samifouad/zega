// One .graph file, three surfaces: the native CLI, `zega start` over HTTP and
// the wasm build the explorer ships (browser/pkg, run here in Node). A graph
// exported by any of them imports into any other and exports again as the
// same bytes (docs/graph-format.md).
import { test, expect } from '@playwright/test';
import { execFile, spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { promisify } from 'node:util';
import init, { ZegaWasm } from '../pkg/zega_wasm.js';

const ZEGA = resolve('../.target/debug/zega');
// Every Value type, float edge cases and unicode names (zega/tests/fixtures).
const GOLDEN = resolve('../zega/tests/fixtures/golden-v1.graph');
const SCHEMA = 'type Player { name: String salary: Int rating?: Float active?: Bool }';

const run = (args, cwd) => promisify(execFile)(ZEGA, args, { cwd, encoding: 'buffer', maxBuffer: 1 << 26 });

async function start(cwd) {
  const child = spawn(ZEGA, ['start', '--port', '0', '--data', 'db'], { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (data) => { stderr += data; });
  const url = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { child.kill(); reject(Error(`CLI startup timed out: ${stderr}`)); }, 15000);
    child.once('exit', (code) => { clearTimeout(timeout); reject(Error(`CLI exited ${code}: ${stderr}`)); });
    createInterface({ input: child.stdout }).once('line', (line) => { clearTimeout(timeout); resolve(line); });
  });
  return {
    url,
    async stop() {
      if (child.exitCode !== null) return;
      const exit = new Promise((resolve) => child.once('exit', resolve));
      child.kill();
      await exit;
    },
  };
}

test('a graph moves between the CLI, the HTTP server and wasm with identical bytes', async () => {
  await init({ module_or_path: await readFile(new URL('../pkg/zega_wasm_bg.wasm', import.meta.url)) });
  await mkdir('.tmp', { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(resolve('.tmp/graph-file-'));
  let server;
  try {
    // 1. The CLI imports the golden file; the server adds to it over ZQL.
    await run(['import', GOLDEN, '--data', 'db'], directory);
    server = await start(directory);
    const zql = await fetch(`${server.url}/zql`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ schema: SCHEMA, query: 'mutation { Player(name: "Ada ✈" && salary: -7 && rating: 0.1 && active: true) { name } }' }),
    });
    expect(zql.ok).toBe(true);
    const served = new Uint8Array(await (await fetch(`${server.url}/graph`)).arrayBuffer());
    const servedJson = (await (await fetch(`${server.url}/graph`, { headers: { Accept: 'application/json' } })).json()).result;
    await server.stop();

    // 2. The CLI exports the same bytes the server streamed.
    const exported = new Uint8Array((await run(['export', '-', '--data', 'db'], directory)).stdout);
    expect(exported.length).toBeGreaterThan(1000);
    expect(Buffer.compare(exported, served)).toBe(0);

    // 3. wasm imports it and exports it again: identical bytes, same graph.
    const db = new ZegaWasm();
    const summary = JSON.parse(db.importGraph(exported));
    expect(summary).toMatchObject({ format_version: 1, nodes: 4, relationships: 2 });
    expect(Buffer.compare(db.exportGraph(), exported)).toBe(0);
    expect(JSON.parse(db.graph())).toEqual(servedJson);

    // 4. And back: wasm writes, the CLI imports, the CLI exports the same bytes.
    db.run(SCHEMA, 'mutation { Player(name: "from wasm" && salary: 1) { name } }');
    const fromWasm = db.exportGraph(null, JSON.stringify({ licence: 'CC0-1.0' }));
    const file = join(directory, 'from-wasm.graph');
    await writeFile(file, fromWasm);
    await run(['import', file, '--data', 'db2'], directory);
    const back = new Uint8Array((await run(['export', '-', '--data', 'db2', '--meta', 'licence=CC0-1.0'], directory)).stdout);
    expect(Buffer.compare(back, fromWasm)).toBe(0);

    // 5. A damaged file changes nothing in wasm either.
    const before = db.exportGraph();
    expect(() => db.importGraph(fromWasm.subarray(0, fromWasm.length - 3))).toThrow(/truncated \.graph file/);
    expect(Buffer.compare(db.exportGraph(), before)).toBe(0);
    db.free();
  } finally {
    await server?.stop();
    await rm(directory, { recursive: true, force: true });
  }
});
