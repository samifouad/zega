// The benchmark itself, shared by the Durable Object (which calls its
// container) and the local Docker dry run (which calls a published port).
// `call(path, init)` must behave like fetch against the zega server.

export const SCHEMA =
  'schema { type Item { n: Int name: String city: String score: Int at: Point from (lat, lon) next -> Item[] } } unique { Item { n } }';

// Five short fields per node: n, name, city, score and a Point (ZQL orders
// only by distance, so the heavy query needs one). Three `next` links each.
export const CITIES = 50;
export const LIMIT_MS = 2000;
const WRITE_BASE = 1_000_000_000;

export function targets(i) {
  if (i < 9) return [i + 1, i + 2, i + 3];
  return [i - 1, i - 7, Number((BigInt(i) * 2654435761n) % BigInt(i - 8))];
}

export function nodesCsv(from, to) {
  let out = 'n,name,city,score,lat,lon\n';
  for (let i = from; i < to; i++) {
    out += `${i},name${i},c${i % CITIES},${(i * 37) % 1000},${(51 + (i % 1000) / 10000).toFixed(4)},${(-114 - (i % 997) / 10000).toFixed(4)}\n`;
  }
  return out;
}

// Links of nodes [from, to). Nodes 0..8 link forward, so the first batch
// must hold at least 12 nodes; every other target already exists.
export function linksCsv(from, to) {
  let out = 'from,to\n';
  for (let i = from; i < to; i++) for (const t of targets(i)) out += `${i},${t}\n`;
  return out;
}

export function loadDocument() {
  return `${SCHEMA}
mutation csv ["nodes.csv"] { Item(n: $n && name: $name && city: $city && score: $score) { n } }
mutation csv ["links.csv"] { Item(n: $from) { next -> link Item(n: $to) { n } } }`;
}

export function stats(samples) {
  const sorted = samples.toSorted((a, b) => a - b);
  const at = q => (sorted.length ? sorted[Math.ceil(sorted.length * q) - 1] : null);
  const round = v => (v == null ? null : Math.round(v * 100) / 100);
  return {
    samples: sorted.length,
    p50Ms: round(at(0.5)),
    p99Ms: round(at(0.99)),
    maxMs: round(sorted.at(-1)),
    minMs: round(sorted[0]),
    over2s: sorted.filter(v => v > LIMIT_MS).length,
  };
}

// Deterministic keys so every size reads the same nodes.
function* keys(seed, span) {
  let x = seed >>> 0 || 1;
  for (;;) {
    x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
    yield x % span;
  }
}

export class Bench {
  /** @param call (path, init) => Promise<Response>; @param store {get, put} async */
  constructor(call, store) {
    this.call = call;
    this.store = store;
  }

  async json(path, init) {
    const start = performance.now();
    const response = await this.call(path, init);
    const text = await response.text();
    const ms = performance.now() - start;
    let body;
    try {
      body = JSON.parse(text);
    } catch {
      throw new Error(`HTTP ${response.status} from ${path}: ${text.slice(0, 300)}`);
    }
    if (!response.ok || body.ok === false) {
      throw new Error(`HTTP ${response.status} from ${path}: ${JSON.stringify(body).slice(0, 300)}`);
    }
    return { body, ms };
  }

  zql(query, extra = {}) {
    return this.json('/zql', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ schema: SCHEMA, query, ...extra }),
    });
  }

  async mem(reset = false) {
    return (await this.json(`/mem${reset ? '?reset=1' : ''}`)).body;
  }

  /** How many nodes the container holds; 0 after its disk was wiped. */
  async loaded() {
    const loaded = (await this.store.get('loaded')) ?? 0;
    if (loaded === 0) return 0;
    const { body } = await this.zql(`{ Item(n: ${loaded - 1}) { n } }`);
    if (body.result === null) {
      await this.store.put('loaded', 0);
      return 0;
    }
    return loaded;
  }

  async load({ nodes, batch = 1000, maxMs = 20000 }) {
    if (!(nodes > 0)) throw new Error('nodes must be a positive number');
    if (batch < 12) throw new Error('batch must be at least 12');
    const started = performance.now();
    let loaded = await this.loaded();
    const batchMs = [];
    let error = null;
    while (loaded < nodes && performance.now() - started < maxMs) {
      const to = Math.min(nodes, loaded + batch);
      try {
        const { ms } = await this.zql(loadDocument(), {
          document: true,
          sources: { 'nodes.csv': nodesCsv(loaded, to), 'links.csv': linksCsv(loaded, to) },
        });
        batchMs.push(ms);
      } catch (e) {
        error = e.message;
        break;
      }
      loaded = to;
      await this.store.put('loaded', loaded);
    }
    let mem = null;
    try {
      mem = await this.mem();
    } catch (e) {
      error ??= e.message;
    }
    return {
      ok: error === null,
      error,
      done: loaded >= nodes,
      nodes: loaded,
      relationships: loaded * 3,
      target: nodes,
      batches: stats(batchMs),
      elapsedMs: Math.round(performance.now() - started),
      mem,
    };
  }

  async loop(kind, { n = 200, seed = 7 } = {}) {
    const loaded = await this.loaded();
    if (kind !== 'write' && loaded === 0) throw new Error('graph is empty; load first');
    const pick = keys(seed, Math.max(loaded, 1));
    const samples = [];
    const errors = [];
    let writes = (await this.store.get('writes')) ?? 0;
    for (let i = 0; i < n; i++) {
      const k = pick.next().value;
      const city = `c${k % CITIES}`;
      const query = {
        read: `{ Item(n: ${k}) { n name city score at } }`,
        hop2: `{ Item(n: ${k}) { n next -> Item { n next -> Item { n } } } }`,
        heavy: `{ Item(city: "${city}" && score >= 900) order by @distance(at, @point(51.05, -114.05)) limit 10 { n name score } }`,
        write: `mutation { Item(n: ${WRITE_BASE + writes} && name: "w${writes}" && city: "${city}" && score: ${k % 1000} && at: @point(51.0, -114.0)) { n } }`,
      }[kind];
      if (!query) throw new Error(`unknown bench ${kind}`);
      try {
        const { ms, body } = await this.zql(query);
        if (kind !== 'write' && kind !== 'heavy' && body.result === null) throw new Error(`node ${k} missing`);
        samples.push(ms);
        if (kind === 'write') writes++;
      } catch (e) {
        errors.push(e.message);
      }
    }
    if (kind === 'write') await this.store.put('writes', writes);
    return { ok: errors.length === 0, bench: kind, nodes: loaded, ...stats(samples), errors: errors.length, firstError: errors[0] ?? null };
  }

  async measured(path) {
    const { body, ms } = await this.json(path, { method: 'POST' });
    return { ok: true, nodes: await this.store.get('loaded') ?? 0, serverMs: Math.round(body.ms * 100) / 100, roundTripMs: Math.round(ms * 100) / 100, before: body.before, after: body.after, disk: body.disk };
  }

  snapshot() {
    return this.measured('/snapshot');
  }

  reload() {
    return this.measured('/reload');
  }

  /** Polls until the container answers a query; returns ms and attempts. */
  async firstAnswer(timeoutMs = 120000) {
    const start = performance.now();
    let attempts = 0;
    let lastError = null;
    while (performance.now() - start < timeoutMs) {
      attempts++;
      try {
        await this.zql('{ Item(n: 0) { n } }');
        return { ms: Math.round(performance.now() - start), attempts };
      } catch (e) {
        lastError = e.message;
        await new Promise(resolve => setTimeout(resolve, 25));
      }
    }
    throw new Error(`no answer within ${timeoutMs} ms: ${lastError}`);
  }
}
