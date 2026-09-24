// SPIKE: option B, the benchmark graph and queries over D1.
//
// Two ways to run each query, so the report can bracket option B:
// - mode=engine:   one D1 call per storage call the engine makes (what the
//                  storage seam does today: index, node, adjacency, node...),
//                  awaited in order. Round trips = storage calls.
// - mode=compiled: the whole query as ONE SQL statement with joins (what a
//                  ZQL-to-SQL compiler would send). One round trip.
// Writes are one D1 batch (a transaction) in both modes.
//
// The graph is generated with the same splitmix64 as ../../src/gen.rs, so
// node i has the same fields and relationships as in the Durable Object.
// Tables are suffixed per graph (/d1/<graph>/...).

const M = (1n << 64n) - 1n;
function mix(x) {
  x = (x + 0x9E3779B97F4A7C15n) & M;
  x = ((x ^ (x >> 30n)) * 0xBF58476D1CE4E5B9n) & M;
  x = ((x ^ (x >> 27n)) * 0x94D049BB133111EBn) & M;
  return x ^ (x >> 31n);
}
function node(i, n) {
  const r = mix(BigInt(i));
  return {
    handle: `p${i}`,
    name: `n${(r & 0xffffffffn).toString(16).padStart(8, "0")}`,
    age: 18 + Number((r >> 32n) % 72n),
    city: `c${(r >> 40n) % 100n}`,
    rank: Number((r >> 16n) % BigInt(Math.max(Math.floor(n / 10), 1))),
  };
}
function targets(i, n) {
  const out = [];
  let k = 0n;
  while (out.length < Math.min(3, n - 1)) {
    const t = Number(1n + (mix((BigInt(i) * 31n + k + 0xABCDn) & M) % BigInt(n)));
    k++;
    if (t !== i && !out.includes(t)) out.push(t);
  }
  return out;
}

const schema = (g) => [
  `CREATE TABLE IF NOT EXISTS node_${g}(id INTEGER PRIMARY KEY, props TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS rel_${g}(id INTEGER PRIMARY KEY, src INTEGER NOT NULL, dst INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS adj_${g}(node INTEGER NOT NULL, dk INTEGER NOT NULL, other INTEGER NOT NULL, rel INTEGER NOT NULL, PRIMARY KEY(node, dk, other, rel)) WITHOUT ROWID`,
  `CREATE TABLE IF NOT EXISTS handle_${g}(key TEXT PRIMARY KEY, id INTEGER NOT NULL) WITHOUT ROWID`,
  `CREATE TABLE IF NOT EXISTS rank_${g}(key REAL NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(key, id)) WITHOUT ROWID`,
];

function rowsInsert(db, table, cols, rows) {
  const per = Math.floor(100 / cols.length);
  const out = [];
  for (let s = 0; s < rows.length; s += per) {
    const chunk = rows.slice(s, s + per);
    const marks = chunk.map(() => `(${cols.map(() => "?").join(",")})`).join(",");
    out.push(db.prepare(`INSERT OR IGNORE INTO ${table}(${cols.join(",")}) VALUES ${marks}`).bind(...chunk.flat()));
  }
  return out;
}

async function seed(db, g, lo, hi, n) {
  await db.batch(schema(g).map((s) => db.prepare(s)));
  const nodes = [], rels = [], adj = [], handles = [], ranks = [];
  for (let i = lo; i <= hi; i++) {
    const p = node(i, n);
    nodes.push([i, JSON.stringify(p)]);
    handles.push([p.handle, i]);
    ranks.push([p.rank, i]);
    targets(i, n).forEach((to, k) => {
      const id = (i - 1) * 3 + k + 1;
      rels.push([id, i, to]);
      adj.push([i, 0, to, id], [to, 1, i, id]);
    });
  }
  const stmts = [
    ...rowsInsert(db, `node_${g}`, ["id", "props"], nodes),
    ...rowsInsert(db, `rel_${g}`, ["id", "src", "dst"], rels),
    ...rowsInsert(db, `adj_${g}`, ["node", "dk", "other", "rel"], adj),
    ...rowsInsert(db, `handle_${g}`, ["key", "id"], handles),
    ...rowsInsert(db, `rank_${g}`, ["key", "id"], ranks),
  ];
  return stmts;
}

// Runs D1 calls and keeps what they cost.
class Meter {
  constructor(db) { this.db = db; this.trips = 0; this.read = 0; this.written = 0; this.sqlMs = 0; this.size = null; }
  note(res) {
    for (const r of Array.isArray(res) ? res : [res]) {
      this.read += r.meta?.rows_read ?? 0;
      this.written += r.meta?.rows_written ?? 0;
      this.sqlMs += r.meta?.duration ?? 0;
      if (r.meta?.size_after != null) this.size = r.meta.size_after;
    }
  }
  async all(sql, ...args) { this.trips++; const r = await this.db.prepare(sql).bind(...args).all(); this.note(r); return r.results; }
  async batch(stmts) { this.trips++; const r = await this.db.batch(stmts); this.note(r); return r; }
}

const pick = (props, names) => Object.fromEntries(names.map((k) => [k, props[k] ?? null]));

async function engine(m, g, shape, i, n) {
  const nodeById = async (id) => {
    const rows = await m.all(`SELECT props FROM node_${g} WHERE id = ?`, id);
    return rows.length ? JSON.parse(rows[0].props) : null;
  };
  const byHandle = async (h) => {
    const ids = await m.all(`SELECT id FROM handle_${g} WHERE key = ?`, h);
    return ids.length ? [ids[0].id, await nodeById(ids[0].id)] : [null, null];
  };
  const out = async (id) => (await m.all(`SELECT other FROM adj_${g} WHERE node = ? AND dk = 0 ORDER BY other, rel`, id)).map((r) => r.other);
  switch (shape) {
    case "point": return (await byHandle(`p${i}`))[1];
    case "one_hop": {
      const [id, p] = await byHandle(`p${i}`);
      const follows = [];
      for (const o of await out(id)) follows.push(pick(await nodeById(o), ["handle", "name"]));
      return { handle: p.handle, follows };
    }
    case "two_hop": {
      const [id, p] = await byHandle(`p${i}`);
      const follows = [];
      for (const o of await out(id)) {
        const q = await nodeById(o);
        const inner = [];
        for (const o2 of await out(o)) inner.push(pick(await nodeById(o2), ["handle", "name"]));
        follows.push({ handle: q.handle, follows: inner });
      }
      return { handle: p.handle, follows };
    }
    case "filter": {
      const ids = await m.all(`SELECT id FROM rank_${g} WHERE key >= ? AND key <= ?`, i, i + 1);
      const rows = [];
      for (const { id } of ids.sort((a, b) => a.id - b.id)) rows.push(pick(await nodeById(id), ["handle", "name", "age"]));
      return rows;
    }
    case "scan_limit":
    case "scan_all": {
      const want = shape === "scan_all" ? ["name", "nobody"] : ["city", `c${i % 100}`];
      const rows = [];
      let after = 0;
      while (rows.length < 10) {
        const page = await m.all(`SELECT id, props FROM node_${g} WHERE id > ? ORDER BY id LIMIT 256`, after);
        if (!page.length) break;
        after = page[page.length - 1].id;
        for (const r of page) {
          const p = JSON.parse(r.props);
          if (p[want[0]] === want[1] && rows.length < 10) rows.push(pick(p, ["handle", "name"]));
        }
      }
      return rows;
    }
  }
  return null;
}

async function compiled(m, g, shape, i) {
  const j = (s) => JSON.parse(s);
  switch (shape) {
    case "point": {
      const r = await m.all(`SELECT n.props FROM handle_${g} h JOIN node_${g} n ON n.id = h.id WHERE h.key = ?`, `p${i}`);
      return r.length ? j(r[0].props) : null;
    }
    case "one_hop": {
      const r = await m.all(
        `SELECT n.props AS root, m.props AS hop FROM handle_${g} h JOIN node_${g} n ON n.id = h.id
         LEFT JOIN adj_${g} a ON a.node = n.id AND a.dk = 0 LEFT JOIN node_${g} m ON m.id = a.other
         WHERE h.key = ? ORDER BY a.other, a.rel`, `p${i}`);
      return r.length ? { handle: j(r[0].root).handle, follows: r.filter((x) => x.hop).map((x) => pick(j(x.hop), ["handle", "name"])) } : null;
    }
    case "two_hop": {
      const r = await m.all(
        `SELECT n.props AS root, a.other AS mid, m.props AS midp, m2.props AS leaf
         FROM handle_${g} h JOIN node_${g} n ON n.id = h.id
         LEFT JOIN adj_${g} a ON a.node = n.id AND a.dk = 0 LEFT JOIN node_${g} m ON m.id = a.other
         LEFT JOIN adj_${g} a2 ON a2.node = m.id AND a2.dk = 0 LEFT JOIN node_${g} m2 ON m2.id = a2.other
         WHERE h.key = ? ORDER BY a.other, a.rel, a2.other, a2.rel`, `p${i}`);
      if (!r.length) return null;
      const follows = [];
      for (const x of r) {
        if (!x.midp) continue;
        let last = follows[follows.length - 1];
        if (!last || last._id !== x.mid) follows.push(last = { _id: x.mid, handle: j(x.midp).handle, follows: [] });
        if (x.leaf) last.follows.push(pick(j(x.leaf), ["handle", "name"]));
      }
      return { handle: j(r[0].root).handle, follows: follows.map(({ _id, ...rest }) => rest) };
    }
    case "filter": {
      const r = await m.all(`SELECT n.props FROM rank_${g} k JOIN node_${g} n ON n.id = k.id WHERE k.key >= ? AND k.key <= ? ORDER BY n.id`, i, i + 1);
      return r.map((x) => pick(j(x.props), ["handle", "name", "age"]));
    }
    case "scan_limit":
    case "scan_all": {
      const [f, v] = shape === "scan_all" ? ["name", "nobody"] : ["city", `c${i % 100}`];
      const r = await m.all(`SELECT props FROM node_${g} WHERE json_extract(props, '$.${f}') = ? ORDER BY id LIMIT 10`, v);
      return r.map((x) => pick(j(x.props), ["handle", "name"]));
    }
  }
  return null;
}

async function write(m, g, shape, i, n) {
  const db = m.db;
  if (shape === "create") {
    const props = { handle: `new${i}`, name: "fresh", age: 30, city: "c1", rank: 5 };
    // One batch = one transaction. The handle table's primary key refuses a
    // duplicate and rolls the whole batch back.
    const id = `(SELECT COALESCE(MAX(id), 0) + 1 FROM node_${g})`;
    await m.batch([
      db.prepare(`INSERT INTO handle_${g}(key, id) VALUES (?, ${id})`).bind(props.handle),
      db.prepare(`INSERT INTO node_${g}(id, props) SELECT id, ? FROM handle_${g} WHERE key = ?`).bind(JSON.stringify(props), props.handle),
      db.prepare(`INSERT INTO rank_${g}(key, id) SELECT 5, id FROM handle_${g} WHERE key = ?`).bind(props.handle),
    ]);
    return { handle: props.handle, rank: 5 };
  }
  // link: new<i> follows 3 existing nodes, found by handle, in one batch.
  const to = [0, 1, 2].map((k) => Number(1n + (mix(BigInt(i + k)) % BigInt(n))));
  const stmts = [];
  for (const t of to) {
    const src = `(SELECT id FROM handle_${g} WHERE key = 'new${i}')`;
    const dst = `(SELECT id FROM handle_${g} WHERE key = 'p${t}')`;
    const rel = `(SELECT COALESCE(MAX(id), 0) FROM rel_${g})`;
    stmts.push(
      db.prepare(`INSERT INTO rel_${g}(id, src, dst) SELECT ${rel} + 1, ${src}, ${dst}`),
      db.prepare(`INSERT INTO adj_${g} SELECT ${src}, 0, ${dst}, ${rel}`),
      db.prepare(`INSERT INTO adj_${g} SELECT ${dst}, 1, ${src}, ${rel}`),
    );
  }
  await m.batch(stmts);
  return { handle: `new${i}`, follows: to.map((t) => ({ handle: `p${t}` })) };
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const parts = url.pathname.split("/");
    if (parts.length !== 4 || parts[1] !== "d1" || !/^[a-z0-9]{1,32}$/.test(parts[2])) {
      return new Response("expected /d1/<graph>/<seed|q|stats>", { status: 404 });
    }
    const g = parts[2];
    const num = (k, d = 0) => Number(url.searchParams.get(k) ?? d);
    const m = new Meter(env.DB);
    const t0 = Date.now();
    try {
      let result;
      if (parts[3] === "seed") {
        const stmts = await seed(env.DB, g, num("lo"), num("hi"), num("n"));
        for (let s = 0; s < stmts.length; s += 200) await m.batch(stmts.slice(s, s + 200));
        result = { seeded: [num("lo"), num("hi")] };
      } else if (parts[3] === "stats") {
        const count = async (t) => (await m.all(`SELECT count(*) AS c FROM ${t}_${g}`))[0].c;
        result = { nodes: await count("node"), relationships: await count("rel"), adjacency_rows: await count("adj") };
        const probe = await env.DB.prepare(`SELECT 1`).run();
        result.database_bytes = probe.meta?.size_after ?? null;
      } else if (parts[3] === "q") {
        const shape = url.searchParams.get("shape");
        const mode = url.searchParams.get("mode") ?? "engine";
        if (shape === "create" || shape === "link") result = await write(m, g, shape, num("i"), num("n", 1));
        else if (mode === "compiled") result = await compiled(m, g, shape, num("i"));
        else result = await engine(m, g, shape, num("i"), num("n", 1));
      } else {
        return new Response("not found", { status: 404 });
      }
      const cost = { round_trips: m.trips, billed_rows_read: m.read, billed_rows_written: m.written, d1_sql_ms: Math.round(m.sqlMs * 1000) / 1000 };
      return Response.json({ ok: true, result, cost, database_bytes: m.size }, { headers: { "x-d1-ms": String(Date.now() - t0) } });
    } catch (e) {
      return Response.json({ ok: false, error: String(e) }, { status: 500 });
    }
  },
};
