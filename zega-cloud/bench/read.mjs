import { options, zql, query, quantiles } from './common.mjs';
const { endpoint, graph, count } = options();
const samples = Number.isFinite(count) ? count : 10000;
const times = [];
let last;
try {
  for (let i = 0; i < 100; i++) {
    const warm = await zql(endpoint, graph, query(String(i)));
    if (warm.value?.key !== String(i)) throw new Error('load graph before measuring reads');
  }
  for (let i = 0; i < samples; i++) {
    last = await zql(endpoint, graph, query(String(i % 100)));
    if (last.value?.key !== String(i % 100)) throw new Error('point query returned the wrong row');
    times.push(last.ms);
  }
  console.log(JSON.stringify({ benchmark: 'read', ok: true, endpoint, graph, ...quantiles(times), heapBytes: last.heapBytes, peakHeapBytes: last.peakHeapBytes, wasmBytes: last.wasmBytes }));
} catch (error) { console.log(JSON.stringify({ benchmark: 'read', ok: false, endpoint, graph, completed: times.length, error: error.message })); process.exitCode = 1; }
