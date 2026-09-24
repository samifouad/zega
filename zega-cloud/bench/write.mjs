import { randomUUID } from 'node:crypto';
import { options, zql, quantiles } from './common.mjs';
const { endpoint, graph, count } = options();
const samples = Number.isFinite(count) ? count : 2000;
const run = randomUUID();
const times = [];
let last;
try {
  for (let i = 0; i < samples; i++) {
    last = await zql(endpoint, graph, `mutation { Record(key: "0") set value: ${i} { key value } }`, `write-${run}-${i}`);
    if (last.value?.key !== '0') throw new Error('load graph before measuring writes');
    times.push(last.ms);
  }
  console.log(JSON.stringify({ benchmark: 'write', ok: true, endpoint, graph, ...quantiles(times), heapBytes: last.heapBytes, peakHeapBytes: last.peakHeapBytes, wasmBytes: last.wasmBytes }));
} catch (error) { console.log(JSON.stringify({ benchmark: 'write', ok: false, endpoint, graph, completed: times.length, error: error.message })); process.exitCode = 1; }
