import { performance } from 'node:perf_hooks';
export const schema = 'schema { type Record { key: String value: Int payload: String } } unique { Record { key } }';
export const query = key => `query { Record(key: "${key}") { key value } }`;
export function options() {
  const endpoint = process.argv[2]?.replace(/\/$/, '');
  if (!endpoint || !/^https?:\/\//.test(endpoint)) throw new Error('usage: node <script>.mjs ENDPOINT [graph] [size-or-count]');
  return { endpoint, graph: process.argv[3] ?? 'bench', count: Number(process.argv[4]) };
}
export function metrics(headers) {
  const n = key => headers.has(key) ? Number(headers.get(key)) : null;
  return { coldStartMs: n('x-zega-cold-start-ms'), heapBytes: n('x-zega-heap-bytes'), peakHeapBytes: n('x-zega-peak-heap-bytes'), wasmBytes: n('x-zega-wasm-bytes'), walSeq: n('x-zega-wal-seq') };
}
export async function post(endpoint, graph, action, body = {}, id, extra = {}) {
  const start = performance.now();
  const response = await fetch(`${endpoint}/g/${encodeURIComponent(graph)}/${action}`, { method: 'POST', headers: { 'content-type': 'application/json', ...(id ? { 'Request-Id': id } : {}), ...extra }, body: JSON.stringify(body), signal: AbortSignal.timeout(120000) });
  const text = await response.text();
  let value;
  try { value = JSON.parse(text); } catch { throw new Error(`HTTP ${response.status}: ${text.slice(0, 500)}`); }
  if (!response.ok || !value.ok) throw new Error(`HTTP ${response.status}: ${JSON.stringify(value)}`);
  return { value: value.result, ms: performance.now() - start, ...metrics(response.headers) };
}
export const zql = (endpoint, graph, q, id) => post(endpoint, graph, 'zql', { schema, query: q }, id);
export const control = (endpoint, graph, action) => post(endpoint, graph, `_spike-${action}`);
export async function evict(endpoint, graph) {
  try {
    const r = await fetch(`${endpoint}/g/${encodeURIComponent(graph)}/_spike-evict`, { method: 'POST', signal: AbortSignal.timeout(10000) });
    if (r.status === 404) throw new Error('SPIKE_CONTROLS must be true for eviction/bench');
    if (r.ok) throw new Error('eviction unexpectedly succeeded without aborting');
    await r.arrayBuffer();
  } catch (e) { if (!/fetch failed|socket|network/i.test(e.message)) throw e; }
}
export function quantiles(values) {
  const sorted = values.toSorted((a, b) => a - b);
  return { samples: sorted.length, p50Ms: sorted[Math.ceil(sorted.length * .5) - 1], p99Ms: sorted[Math.ceil(sorted.length * .99) - 1], minMs: sorted[0], maxMs: sorted.at(-1) };
}
