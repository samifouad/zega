// Shared by client.mjs (runs inside Fly) and mac.mjs (runs on Sami's Mac):
// the sizes, the run limits, a paced Bench and the report helpers.
import { createHash } from 'node:crypto';
import { Bench, stats } from '../containers-bench/src/bench.js';

export const MiB = 1024 * 1024;

// Fly Machine sizes under test, with the largest graph each may be loaded to.
// The caps keep the run modest (no push-to-failure); capacity beyond the cap
// is extrapolated from measured memory per node and labelled an estimate.
export const SIZES = {
  s1x: { vm: 'shared-cpu-1x', memoryMb: 256, cap: 50_000 },
  s2x: { vm: 'shared-cpu-2x', memoryMb: 512, cap: 100_000 },
  s4x: { vm: 'shared-cpu-4x', memoryMb: 1024, cap: 200_000 },
  p1x: { vm: 'performance-1x', memoryMb: 2048, cap: 200_000 },
};

// Run limits (Sami, 2026-09-24): sequential requests only, at most 20 per
// second against any machine, short pauses between phases.
export const LIMITS = {
  maxRps: 20,
  pauseMs: 3000,
  n: 100,
  heavyN: 10,
  macN: 30,
  macGapMs: 500,
};

export const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

/** Spaces request starts at least 1000/rps ms apart. */
export function pacer(rps) {
  const gap = 1000 / rps;
  let next = 0;
  return async () => {
    const now = performance.now();
    if (next > now) await sleep(next - now);
    next = Math.max(now, next) + gap;
  };
}

/**
 * bench.js's Bench, with every request paced and authorized. The pause is
 * taken before Bench starts its timer, so it never shows in a latency.
 */
export class PacedBench extends Bench {
  constructor({ url, token, rps = LIMITS.maxRps, headers = {}, timeoutMs = 600000 }) {
    const store = new Map();
    super(
      (path, init = {}) =>
        fetch(`${url}${path}`, {
          ...init,
          headers: { ...headers, ...(init.headers ?? {}), authorization: `Bearer ${token}` },
          signal: AbortSignal.timeout(timeoutMs),
        }),
      { get: async k => store.get(k), put: async (k, v) => void store.set(k, v) },
    );
    this.url = url;
    this.pace = pacer(rps);
  }
  async json(path, init) {
    await this.pace();
    return super.json(path, init);
  }
  async wipe() {
    const result = await this.measured('/wipe');
    await this.store.put('loaded', 0);
    return { ...result, nodes: 0 };
  }
  /** The largest loaded key, found by search (loads come in batches of 1,000). */
  async findLoaded(batch = 1000) {
    const has = async k => (await this.zql(`{ Item(n: ${k * batch - 1}) { n } }`)).body.result !== null;
    if (!(await has(1))) return 0;
    let lo = 1;
    let hi = 2;
    while (await has(hi)) [lo, hi] = [hi, hi * 2];
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (await has(mid)) lo = mid;
      else hi = mid;
    }
    await this.store.put('loaded', lo * batch);
    return lo * batch;
  }
  /**
   * A hash of 22 nodes (the first, the last and 20 spread between) with their
   * fields and `next` targets, to show a graph survived a restart or resize.
   */
  async fingerprint() {
    const loaded = (await this.store.get('loaded')) ?? 0;
    if (loaded === 0) return { nodes: 0, found: 0, hash: null };
    const keys = [0, loaded - 1, ...Array.from({ length: 20 }, (_, i) => Math.floor(((i + 1) * loaded) / 22))];
    const rows = [];
    for (const k of keys) {
      const { body } = await this.zql(`{ Item(n: ${k}) { n name city score next -> Item { n } } }`);
      rows.push(canonical(body.result));
    }
    const found = rows.filter(r => r !== 'null').length;
    return { nodes: loaded, found, hash: createHash('sha256').update(rows.join('\n')).digest('hex').slice(0, 16) };
  }
}

// Stable JSON: object keys sorted, arrays sorted by their JSON.
function canonical(value) {
  const norm = v =>
    Array.isArray(v)
      ? v.map(norm).sort((a, b) => (JSON.stringify(a) < JSON.stringify(b) ? -1 : 1))
      : v && typeof v === 'object'
        ? Object.fromEntries(Object.keys(v).sort().map(k => [k, norm(v[k])]))
        : v;
  return JSON.stringify(norm(value));
}

/**
 * What the process can use: the memory available when the graph was empty
 * plus the process's own RSS then, or the cgroup limit if lower (Docker).
 */
export function budgetOf(mem) {
  const avail = (mem?.machine?.memAvailable ?? Infinity) + (mem?.memory?.rss ?? 0);
  return Math.min(avail, mem?.machine?.cgroupMax ?? Infinity);
}

/**
 * Nodes that would fit, from the biggest measured step: the peak RSS over
 * the empty-graph baseline, per node, against 90% and 100% of the budget.
 * An extrapolation, not a measurement.
 */
export function estimateCapacity({ nodes, peak, baseline, budget }) {
  if (!(nodes > 0) || !(peak > baseline) || !Number.isFinite(budget)) return null;
  const perNode = (peak - baseline) / nodes;
  return {
    bytesPerNode: Math.round(perNode),
    at90: Math.floor((0.9 * budget - baseline) / perNode),
    at100: Math.floor((budget - baseline) / perNode),
  };
}

export const ms = v => (v == null ? '–' : `${Math.round(v * 10) / 10}`);
export const mb = v => (v == null ? '–' : `${Math.round(v / MiB)}`);
export const count = v => (v == null ? '–' : v.toLocaleString('en-US'));
export const pair = r => (r == null ? '–' : r.ok === false && !r.samples ? 'failed' : `${ms(r.p50Ms)} / ${ms(r.p99Ms)}${r.over2s ? ` (${r.over2s} > 2 s)` : ''}${r.errors ? ` (${r.errors} errors)` : ''}`);

export function markdown(columns, rows) {
  let out = `| | ${columns.join(' | ')} |\n|---|${columns.map(() => '---').join('|')}|\n`;
  for (const [label, cell] of rows) out += `| ${label} | ${columns.map(c => cell(c)).join(' | ')} |\n`;
  return out;
}

export { stats };

/** `a=1,b=2` into [['a','1'],['b','2']]. */
export function pairs(value) {
  return value.split(',').filter(Boolean).map(item => {
    const at = item.indexOf('=');
    if (at < 1) throw new Error(`expected name=value, got ${item}`);
    return [item.slice(0, at), item.slice(at + 1)];
  });
}

export function flags(argv, defaults, booleans = []) {
  const out = { ...defaults };
  for (let i = 0; i < argv.length; i++) {
    if (!argv[i].startsWith('--')) throw new Error(`unexpected argument ${argv[i]}`);
    const key = argv[i].slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    if (!(key in defaults) && !booleans.includes(key)) throw new Error(`unknown flag ${argv[i]}`);
    if (booleans.includes(key)) out[key] = true;
    else {
      const value = argv[++i];
      if (value === undefined) throw new Error(`${argv[i - 1]} needs a value`);
      out[key] = typeof defaults[key] === 'number' ? Number(value) : value;
    }
  }
  return out;
}
