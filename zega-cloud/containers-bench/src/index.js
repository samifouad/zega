// zega on Cloudflare Containers: one Container class per instance type, each
// fronted by its own Durable Object. Routes, all behind the admin token
// (src/front.js, which also answers CORS for explorer2.zega.dev):
//
//   /c/<lite|basic|std1>/<graph>/bench/<load|read|write|hop2|heavy|snapshot|reload|cold|mem>
//   /c/<lite|basic|std1>/<graph>/<anything else>   forwarded to the container (e.g. /zql)
//
// Bench loops run inside the Durable Object, so their timings are the
// DO-to-container hop plus zega, not the client's network.
import { Container } from '@cloudflare/containers';
import { Bench } from './bench.js';
import { handle, json } from './front.js';

function number(params, name, fallback) {
  const value = params.get(name);
  if (value === null) return fallback;
  const n = Number(value);
  if (!Number.isFinite(n) || n < 0) throw new Error(`${name} must be a non-negative number`);
  return n;
}

class ZegaBench extends Container {
  defaultPort = 8080;
  sleepAfter = '5m';
  enableInternet = false;

  bench() {
    const store = {
      get: key => this.ctx.storage.get(key),
      put: (key, value) => this.ctx.storage.put(key, value),
    };
    return new Bench((path, init) => this.containerFetch(`http://container${path}`, init), store);
  }

  async fetch(request) {
    const url = new URL(request.url);
    const [, size, graph, ...rest] = url.pathname.split('/').filter(Boolean);
    if (rest[0] !== 'bench') {
      return this.containerFetch(new Request(`http://container/${rest.join('/')}${url.search}`, request));
    }
    const kind = rest[1];
    const params = url.searchParams;
    const started = performance.now();
    try {
      const bench = this.bench();
      let result;
      switch (kind) {
        case 'load':
          result = await bench.load({
            nodes: number(params, 'nodes', 0),
            batch: number(params, 'batch', 1000),
            maxMs: number(params, 'maxMs', 20000),
          });
          break;
        case 'read':
        case 'write':
        case 'hop2':
        case 'heavy':
          result = await bench.loop(kind, { n: number(params, 'n', 200) });
          break;
        case 'snapshot':
          result = await bench.snapshot();
          break;
        case 'reload':
          result = await bench.reload();
          break;
        case 'mem':
          result = { ok: true, ...(await bench.mem(params.has('reset'))) };
          break;
        case 'cold':
          result = await this.cold(bench);
          break;
        default:
          return json({ ok: false, error: `unknown bench ${kind}` }, 404);
      }
      return json({ size, graph, doMs: Math.round(performance.now() - started), ...result });
    } catch (error) {
      return json({ ok: false, size, graph, bench: kind, error: error.message, doMs: Math.round(performance.now() - started) }, 500);
    }
  }

  // Stop the container (its disk goes with it), then time a fresh start to
  // the first answered query. The graph is empty afterwards.
  async cold(bench) {
    const stopStart = performance.now();
    await this.stop();
    let state = await this.getState();
    let killed = false;
    while (!['stopped', 'stopped_with_code'].includes(state.status)) {
      const waited = performance.now() - stopStart;
      if (waited > 60000) throw new Error(`container still ${state.status} after 60 s`);
      if (waited > 10000 && !killed) {
        // SIGTERM ignored: fall back to SIGKILL, and say so in the result.
        await this.destroy();
        killed = true;
      }
      await new Promise(resolve => setTimeout(resolve, 50));
      state = await this.getState();
    }
    const stopMs = Math.round(performance.now() - stopStart);
    await this.ctx.storage.put('loaded', 0);
    const first = await bench.firstAnswer();
    const mem = await bench.mem();
    return { ok: true, bench: 'cold', stopMs, killed, firstAnswerMs: first.ms, attempts: first.attempts, mem };
  }
}

export class ZegaLite extends ZegaBench {}
export class ZegaBasic extends ZegaBench {}
export class ZegaStd1 extends ZegaBench {}

export default { fetch: handle };
