// The standalone site stores its graph in wasm. The CLI supplies this same UI
// with a native backend; wasm remains the shared parser for editor diagnostics
// and import previews, while every database operation goes over HTTP.
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

class NativeDatabase {
  native = true;
  snapshot = { nodes: [], rels: [] };
  constructor(parser) { this.parser = parser; }
  async request(path, method = 'GET', body) {
    const response = await fetch(path, {
      method,
      // `GET /graph` answers with a .graph file unless JSON is asked for.
      headers: body === undefined ? { Accept: 'application/json' } : { Accept: 'application/json', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || `HTTP ${response.status}`);
    return result.result;
  }
  async refresh() { this.snapshot = await this.request('/graph'); }
  async execute(query, schema, sources, document) {
    try {
      return JSON.stringify(await this.request('/zql', 'POST', {
        schema, query, document,
        sources: sources === undefined ? undefined : JSON.parse(sources),
      }));
    } finally { await this.refresh(); }
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
