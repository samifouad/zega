#!/usr/bin/env node
// Runs the zega Containers benchmark and prints one JSON line per result,
// then a Markdown table.
//
//   Against the deployed Worker (in-DO timings, plus client wall time):
//     node driver/run.mjs --worker https://zega-containers-bench.<account>.workers.dev --token-file ./admin-token
//   Local dry run against Docker with Cloudflare's limits (host timings):
//     node driver/run.mjs --docker zega-bench --sizes lite
//
// Plan, per size (lite first; its capacity is the shared size):
//   1. grow a graph in steps (x --factor); at each step load, snapshot and
//      reload, and stop before the next step is predicted to pass 90% of
//      the memory limit. The last good step is the capacity.
//   2. at the shared size and at the size's own capacity: read, write,
//      2-hop, heavy, snapshot, reload.
//   3. keep growing until the container fails, to record where it breaks.
//   4. cold start: stop the container, time the first answered query.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { Bench } from '../src/bench.js';

const MiB = 1024 * 1024;
const SIZES = {
  lite: { memory: 256 * MiB, cpus: 0.0625 },
  basic: { memory: 1024 * MiB, cpus: 0.25 },
  std1: { memory: 4096 * MiB, cpus: 0.5 },
};

function args() {
  const out = { sizes: 'lite,basic,std1', start: 10000, factor: 1.25, batch: 1000, n: 200, heavyN: 30, maxNodes: Infinity, toFailure: true };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const key = argv[i].replace(/^--/, '');
    const value = argv[++i];
    if (value === undefined) throw new Error(`--${key} needs a value`);
    switch (key) {
      case 'worker': out.worker = value.replace(/\/$/, ''); break;
      case 'token-file': out.tokenFile = value; break;
      case 'docker': out.docker = value; break;
      case 'sizes': out.sizes = value; break;
      case 'start': out.start = Number(value); break;
      case 'factor': out.factor = Number(value); break;
      case 'batch': out.batch = Number(value); break;
      case 'n': out.n = Number(value); break;
      case 'heavy-n': out.heavyN = Number(value); break;
      case 'max-nodes': out.maxNodes = Number(value); break;
      case 'shared': out.shared = Number(value); break;
      case 'to-failure': out.toFailure = value === 'on'; break;
      default: throw new Error(`unknown flag --${key}`);
    }
  }
  if (!out.worker === !out.docker) throw new Error('pass exactly one of --worker URL or --docker IMAGE');
  if (out.worker && !out.tokenFile) throw new Error('--worker needs --token-file');
  out.sizes = out.sizes.split(',');
  for (const size of out.sizes) if (!SIZES[size]) throw new Error(`unknown size ${size}`);
  return out;
}

// Calls the deployed Worker; the bench runs inside the Durable Object.
class WorkerTarget {
  constructor(url, token) {
    this.url = url;
    this.token = token;
    this.where = 'in-DO';
  }
  async run(size, graph, kind, params = {}) {
    const query = new URLSearchParams(params).toString();
    const start = performance.now();
    const response = await fetch(`${this.url}/c/${size}/${graph}/bench/${kind}${query ? `?${query}` : ''}`, {
      headers: { authorization: `Bearer ${this.token}` },
      signal: AbortSignal.timeout(600000),
    });
    const text = await response.text();
    const clientMs = Math.round(performance.now() - start);
    try {
      return { ...JSON.parse(text), clientMs };
    } catch {
      return { ok: false, error: `HTTP ${response.status}: ${text.slice(0, 300)}`, clientMs };
    }
  }
}

// Runs the same Bench code against a local container with the size's limits.
// A new graph gets a new container (a fresh disk), as a new DO would.
class DockerTarget {
  constructor(image) {
    this.image = image;
    this.where = 'docker-host';
    this.current = null;
    this.stores = new Map();
  }
  docker(...argv) {
    return execFileSync('docker', argv, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  }
  name(size) {
    return `zega-bench-${size}`;
  }
  running(size) {
    try {
      return this.docker('inspect', '-f', '{{.State.Running}}', this.name(size)) === 'true';
    } catch {
      return false;
    }
  }
  exitState(size) {
    try {
      return this.docker('inspect', '-f', '{{.State.OOMKilled}} {{.State.ExitCode}}', this.name(size));
    } catch {
      return 'missing';
    }
  }
  fresh(size) {
    const { memory, cpus } = SIZES[size];
    try { this.docker('rm', '-f', this.name(size)); } catch {}
    this.docker('run', '-d', '--name', this.name(size), `--memory=${memory}`, `--memory-swap=${memory}`, `--cpus=${cpus}`, '-p', '127.0.0.1::8080', this.image);
    this.port = this.docker('port', this.name(size), '8080/tcp').split(':').pop();
  }
  async ensure(size, graph) {
    const key = `${size}/${graph}`;
    let restarted = null;
    if (this.current !== key) {
      if (this.current) try { this.docker('rm', '-f', this.name(this.current.split('/')[0])); } catch {}
      this.fresh(size);
      this.current = key;
    } else if (!this.running(size)) {
      restarted = this.exitState(size);
      this.fresh(size);
    }
    if (!this.stores.has(key)) this.stores.set(key, new Map());
    const map = this.stores.get(key);
    const store = { get: async k => map.get(k), put: async (k, v) => void map.set(k, v) };
    const bench = new Bench((path, init) => fetch(`http://127.0.0.1:${this.port}${path}`, { ...init, signal: AbortSignal.timeout(600000) }), store);
    await bench.firstAnswer(30000);
    return { bench, restarted, store };
  }
  async run(size, graph, kind, params = {}) {
    const start = performance.now();
    try {
      const { bench, restarted, store } = await this.ensure(size, graph);
      let result;
      switch (kind) {
        case 'load': result = await bench.load({ nodes: Number(params.nodes), batch: Number(params.batch ?? 1000), maxMs: Number(params.maxMs ?? 20000) }); break;
        case 'read': case 'write': case 'hop2': case 'heavy': result = await bench.loop(kind, { n: Number(params.n ?? 200) }); break;
        case 'snapshot': result = await bench.snapshot(); break;
        case 'reload': result = await bench.reload(); break;
        case 'mem': result = { ok: true, ...(await bench.mem('reset' in params)) }; break;
        case 'cold': {
          const stopStart = performance.now();
          this.docker('rm', '-f', this.name(size));
          const stopMs = Math.round(performance.now() - stopStart);
          const runStart = performance.now();
          this.fresh(size);
          await store.put('loaded', 0);
          const first = await bench.firstAnswer();
          const firstAnswerMs = Math.round(performance.now() - runStart);
          result = { ok: true, bench: 'cold', stopMs, firstAnswerMs, attempts: first.attempts, mem: await bench.mem() };
          break;
        }
        default: throw new Error(`unknown bench ${kind}`);
      }
      if (!result.ok && !this.running(size)) result.container = this.exitState(size);
      return { size, graph, ...result, restartedAfter: restarted, clientMs: Math.round(performance.now() - start) };
    } catch (error) {
      return { ok: false, size, graph, bench: kind, error: error.message, container: this.exitState(size), clientMs: Math.round(performance.now() - start) };
    }
  }
  close() {
    for (const size of Object.keys(SIZES)) try { this.docker('rm', '-f', this.name(size)); } catch {}
  }
}

const results = [];
function emit(line) {
  results.push(line);
  console.log(JSON.stringify(line));
}

const peakOf = r => r?.mem?.memory?.peakRss ?? r?.after?.peakRss ?? null;

async function loadTo(target, size, graph, nodes, o) {
  let last;
  do {
    last = await target.run(size, graph, 'load', { nodes, batch: o.batch });
    emit({ phase: 'load', ...last });
  } while (last.ok && !last.done);
  return last;
}

// One probe step: load to `nodes`, then snapshot and reload, with the peak
// RSS reset before each so every phase reports its own peak.
async function step(target, size, graph, nodes, o) {
  await target.run(size, graph, 'mem', { reset: '1' });
  const load = await loadTo(target, size, graph, nodes, o);
  if (!load.ok) return { ok: false, nodes, failed: 'load', error: load.error };
  const snapshot = await target.run(size, graph, 'snapshot');
  emit({ phase: 'probe-snapshot', ...snapshot });
  if (!snapshot.ok) return { ok: false, nodes, failed: 'snapshot', error: snapshot.error };
  const reload = await target.run(size, graph, 'reload');
  emit({ phase: 'probe-reload', ...reload });
  if (!reload.ok) return { ok: false, nodes, failed: 'reload', error: reload.error };
  const peaks = { load: peakOf(load), snapshot: peakOf(snapshot), reload: peakOf(reload) };
  return { ok: true, nodes, peaks, peak: Math.max(...Object.values(peaks).filter(v => v != null)) };
}

const next = (nodes, o) => Math.max(nodes + o.batch, Math.round((nodes * o.factor) / o.batch) * o.batch);

async function probe(target, size, graph, from, o) {
  const limit = SIZES[size].memory;
  let good = null;
  let nodes = from;
  for (;;) {
    const result = await step(target, size, graph, nodes, o);
    emit({ phase: 'probe', size, graph, ...result, limit });
    if (!result.ok) break;
    good = result;
    const upcoming = next(nodes, o);
    if (upcoming > o.maxNodes) break;
    // Memory grows about linearly with nodes; stop before the next step would
    // pass 90% of the limit, so the graph stays alive for the benches.
    if ((result.peak * upcoming) / nodes > 0.9 * limit) break;
    nodes = upcoming;
  }
  return good;
}

async function benches(target, size, graph, label, o) {
  const out = {};
  for (const [kind, n] of [['read', o.n], ['write', o.n], ['hop2', o.n], ['heavy', o.heavyN]]) {
    out[kind] = await target.run(size, graph, kind, { n });
    emit({ phase: label, ...out[kind] });
  }
  for (const kind of ['snapshot', 'reload']) {
    out[kind] = await target.run(size, graph, kind);
    emit({ phase: label, bench: kind, ...out[kind] });
  }
  return out;
}

async function toFailure(target, size, graph, from, o) {
  let nodes = from;
  for (let i = 0; i < 12; i++) {
    nodes = next(nodes, o);
    if (nodes > o.maxNodes) return { failedAt: null, note: 'stopped at --max-nodes' };
    const result = await step(target, size, graph, nodes, o);
    emit({ phase: 'to-failure', size, graph, ...result });
    if (!result.ok) return result;
  }
  return { failedAt: null, note: 'did not fail within 12 steps' };
}

const ms = v => (v == null ? '–' : `${Math.round(v * 10) / 10}`);
const mb = v => (v == null ? '–' : `${Math.round(v / MiB)}`);
const pair = r => (r?.ok === false ? `failed` : `${ms(r?.p50Ms)} / ${ms(r?.p99Ms)}${r?.over2s ? ` (${r.over2s} > 2 s)` : ''}`);

function table(summary, where) {
  const rows = [
    ['capacity (nodes / relationships)', s => (s.capacity ? `${s.capacity.nodes.toLocaleString('en-US')} / ${(s.capacity.nodes * 3).toLocaleString('en-US')}` : '–')],
    ['peak RSS at capacity: load / snapshot / reload (MiB)', s => (s.capacity ? `${mb(s.capacity.peaks.load)} / ${mb(s.capacity.peaks.snapshot)} / ${mb(s.capacity.peaks.reload)}` : '–')],
    ['first failure', s => (s.failure?.ok === false ? `${s.failure.failed} at ${s.failure.nodes.toLocaleString('en-US')} nodes` : s.failure?.note ?? '–')],
  ];
  for (const at of ['shared', 'own']) {
    const tag = at === 'shared' ? 'shared size' : 'own capacity';
    rows.push([`**at ${tag}** (nodes)`, s => (s[at]?.read?.nodes ?? '–').toLocaleString('en-US')]);
    for (const kind of ['read', 'write', 'hop2', 'heavy']) rows.push([`${kind} p50 / p99 (ms)`, s => pair(s[at]?.[kind])]);
    rows.push(['snapshot (ms, server)', s => ms(s[at]?.snapshot?.serverMs)]);
    rows.push(['reload from disk (ms, server)', s => ms(s[at]?.reload?.serverMs)]);
  }
  rows.push(['cold: container start to first answer (ms)', s => ms(s.cold?.firstAnswerMs)]);
  rows.push(['cold + reload at capacity (ms, estimate)', s => (s.cold?.firstAnswerMs != null && s.own?.reload?.serverMs != null ? ms(s.cold.firstAnswerMs + s.own.reload.serverMs) : '–')]);
  const sizes = Object.keys(summary);
  let out = `\nTimings: ${where}.\n\n| | ${sizes.join(' | ')} |\n|---|${sizes.map(() => '---').join('|')}|\n`;
  for (const [label, cell] of rows) out += `| ${label} | ${sizes.map(size => cell(summary[size])).join(' | ')} |\n`;
  return out;
}

async function main() {
  const o = args();
  const target = o.worker ? new WorkerTarget(o.worker, readFileSync(o.tokenFile, 'utf8').trim()) : new DockerTarget(o.docker);
  const ts = Date.now().toString(36);
  const summary = {};
  try {
    let shared = o.shared;
    const order = [...o.sizes].sort((a, b) => Object.keys(SIZES).indexOf(a) - Object.keys(SIZES).indexOf(b));
    for (const size of order) {
      const s = (summary[size] = {});
      const graph = `g-${size}-${ts}`;
      if (shared == null) {
        // The first (smallest) size sets the shared size: its capacity.
        s.capacity = await probe(target, size, graph, o.start, o);
        if (!s.capacity) throw new Error(`${size} failed its first step at ${o.start} nodes`);
        shared = s.capacity.nodes;
        emit({ phase: 'shared-size', size, nodes: shared, relationships: shared * 3 });
        s.shared = await benches(target, size, graph, 'at-shared', o);
        s.own = s.shared;
      } else {
        const load = await loadTo(target, size, graph, shared, o);
        if (load.ok) s.shared = await benches(target, size, graph, 'at-shared', o);
        s.capacity = await probe(target, size, graph, next(shared, o), o);
        if (s.capacity) s.own = await benches(target, size, graph, 'at-capacity', o);
      }
      if (o.toFailure && s.capacity) s.failure = await toFailure(target, size, graph, s.capacity.nodes, o);
      s.cold = await target.run(size, graph, 'cold');
      emit({ phase: 'cold', ...s.cold });
    }
  } finally {
    if (target.close) target.close();
  }
  console.log(table(summary, target.where === 'in-DO' ? 'measured inside each Durable Object (DO to container and back); clientMs in the JSON lines is the end-to-end wall time' : 'measured from the Docker host through a published port (local preview, not Cloudflare)'));
}

main().catch(error => {
  console.error(error);
  process.exit(1);
});
