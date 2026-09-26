// The standalone site stores its graph in wasm. The CLI supplies this same UI
// with a native backend; wasm remains the shared parser for editor diagnostics
// and import previews, while every database operation goes over HTTP.
// A remote Zega Cloud graph uses the same HTTP backend, pointed at
// api.zega.dev/g/<id> with the graph's API key (RemoteDatabase below).
import { mayWrite } from './zql-edit.js';

export async function connectDatabase(parser) {
  const response = await fetch('/explorer-config.json');
  if (response.status === 404) return parser;
  if (!response.ok) throw new Error(`Cannot configure explorer: HTTP ${response.status}`);
  const config = await response.json();
  if (config.backend !== 'native') throw new Error('Unknown explorer backend');
  const db = new NativeDatabase(parser);
  await db.refresh();
  return db;
}

/** Zega Cloud's router. It allows exactly https://explorer.zega.dev (zegadb/cloud src/router.ts). */
export const REMOTE_API = 'https://api.zega.dev';
/** The router's `/g/<id>/` pattern (zegadb/cloud src/app.ts). Ids are opaque: old and new shapes both match. */
export const GRAPH_ID = /^[a-z0-9-]{1,40}$/;
/** A graph API key (zegadb/cloud src/keys.ts bearerKey). */
export const API_KEY = /^zk_[a-z2-7]{32}$/;

class HttpDatabase {
  native = true;
  snapshot = { nodes: [], rels: [] };
  constructor(parser, base = '') { this.parser = parser; this.base = base; }
  fetchOptions() { return {}; }
  async request(path, method = 'GET', body) {
    const options = this.fetchOptions();
    const response = await fetch(this.base + path, {
      ...options,
      method,
      // `GET /graph` answers with a .graph file unless JSON is asked for.
      headers: {
        ...options.headers,
        ...(body === undefined ? { Accept: 'application/json' } : { Accept: 'application/json', 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || `HTTP ${response.status}`);
    return result.result;
  }
  async refresh() { this.snapshot = await this.request('/graph'); }
  // GET /graph is the whole graph, and on Zega Cloud every call is metered:
  // the snapshot is refreshed after a document or anything that may write
  // (even if it failed part way), never after a read.
  async execute(query, schema, sources, document) {
    const writes = document || mayWrite(query);
    try {
      return JSON.stringify(await this.request('/zql', 'POST', {
        schema, query, document,
        sources: sources === undefined ? undefined : JSON.parse(sources),
      }));
    } finally { if (writes) await this.refresh(); }
  }
  run_with_sources(schema, query, sources) { return this.execute(query, schema, sources, false); }
  apply_with_sources(query, sources) { return this.execute(query, '', sources, true); }
  schema(source) { return this.parser.schema(source); }
  load_locations(source, document) { return this.parser.load_locations(source, document); }
  check(schema, source) { return this.parser.check(schema, source); }
  async vector_view(schema, result, kind, selected, k, threshold) {
    return this.request('/vector-view', 'POST', {
      schema, result: typeof result === 'string' ? JSON.parse(result) : result, kind, selected, k, threshold,
    });
  }
  preview_import(text) { return this.parser.preview_import(text); }
  graph() { return JSON.stringify(this.snapshot); }
  async clear() { await this.request('/graph', 'DELETE'); await this.refresh(); }
  async delete_node(id) { await this.request(`/graph/nodes/${id}`, 'DELETE'); await this.refresh(); }
  async delete_relationship(id) { await this.request(`/graph/relationships/${id}`, 'DELETE'); await this.refresh(); }
  async connect(schema, from, field, to) {
    await this.request('/graph/relationships', 'POST', { schema, from, field, to });
    await this.refresh();
  }
}

/** `zega explorer`: the CLI serves this page and answers /zql and /graph itself, reading local files named in ZQL. */
class NativeDatabase extends HttpDatabase {
  resolvesSources = true;
}

/**
 * A Zega Cloud graph, reached with its API key.
 *
 * The key lives in this object's private field and nowhere else: never in
 * localStorage, sessionStorage, IndexedDB, a cookie, the URL or a log. It is
 * gone on reload, and `forget()` (Disconnect) drops it at once. Requests omit
 * credentials, so no cookie travels with them, and the router allows this
 * page's origin only, without credentials.
 *
 * The graph's machine cannot read files on this computer, so ZQL sources are
 * fetched here in the browser and sent with the query, as in the wasm mode.
 */
export class RemoteDatabase extends HttpDatabase {
  remote = true;
  resolvesSources = false;
  #key;
  constructor(parser, graphId, key) {
    if (!GRAPH_ID.test(graphId)) throw new Error('A graph id is lowercase letters, digits and dashes.');
    if (!API_KEY.test(key)) throw new Error('An API key is zk_ followed by 32 characters.');
    super(parser, `${REMOTE_API}/g/${graphId}`);
    this.graphId = graphId;
    this.#key = key;
  }
  get connected() { return this.#key !== null; }
  forget() { this.#key = null; }
  fetchOptions() {
    if (this.#key === null) throw new Error('Disconnected from the remote graph.');
    return { headers: { Authorization: `Bearer ${this.#key}` }, credentials: 'omit', cache: 'no-store', referrerPolicy: 'no-referrer' };
  }
  async request(path, method, body) {
    try {
      return await super.request(path, method, body);
    } catch (error) {
      // fetch() rejects with a TypeError when the network or CORS stops it; the router's own errors arrive as JSON above.
      if (error instanceof TypeError) throw new Error(`Cannot reach ${REMOTE_API}. Check the connection and try again.`);
      throw error;
    }
  }
}
