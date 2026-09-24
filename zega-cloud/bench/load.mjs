import { performance } from 'node:perf_hooks';
import { options, post, schema, control } from './common.mjs';
const { endpoint, graph, count } = options();
const mb = Number.isFinite(count) ? count : 10;
if (!(mb > 0 && mb <= 1000)) throw new Error('size must be 1..1000 MiB');
const target = Math.round(mb * 1024 * 1024);
const payloadSize = 4096;
let seed = 0x5e6a;
function payload() {
  let out = '';
  for (let i = 0; i < payloadSize; i++) { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; out += String.fromCharCode(65 + (seed >>> 24) % 26); }
  return out;
}
let nodes = 0;
let acknowledgedNodes = 0;
let result;
let peakHeapBytes = 0;
let peakWasmBytes = 0;
const start = performance.now();
try {
  while (nodes * payloadSize < target) {
    const items = [];
    for (let i = 0; i < 32 && nodes * payloadSize < target; i++, nodes++) {
      items.push({ key: String(nodes), value: nodes, payload: payload() });
    }
    result = await post(endpoint, graph, 'zql', { schema, query: 'mutation json ["./rows.json"] { Record(key: $key && value: $value && payload: $payload) { key } }', sources: { './rows.json': JSON.stringify(items) } }, `load-${mb}-${nodes}`);
    acknowledgedNodes = nodes;
    peakHeapBytes = Math.max(peakHeapBytes, result.peakHeapBytes);
    peakWasmBytes = Math.max(peakWasmBytes, result.wasmBytes);
  }
  const stored = await control(endpoint, graph, 'stats');
  console.log(JSON.stringify({ benchmark: 'load', ok: true, endpoint, graph, requestedMiB: mb, seed: '0x5e6a', nodes, payloadBytes: nodes * payloadSize, elapsedMs: performance.now() - start, peakHeapBytes, peakWasmBytes, ...stored.value }));
} catch (error) {
  console.log(JSON.stringify({ benchmark: 'load', ok: false, endpoint, graph, requestedMiB: mb, attemptedNodes: nodes, acknowledgedPayloadBytes: acknowledgedNodes * payloadSize, elapsedMs: performance.now() - start, peakHeapBytes, peakWasmBytes, error: error.message }));
  process.exitCode = 1;
}
