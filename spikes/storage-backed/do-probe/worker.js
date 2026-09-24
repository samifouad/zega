// SPIKE: what one synchronous ctx.storage.sql.exec costs inside a Durable
// Object, for the reads a storage-backed zega makes (node by id, adjacency
// by node). Timed from the client: workerd freezes the clock during a request.
import { DurableObject } from "cloudflare:workers";

export class Graph extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS node(id INTEGER PRIMARY KEY, labels BLOB NOT NULL, props BLOB NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS adj(node INTEGER NOT NULL, dk INTEGER NOT NULL, other INTEGER NOT NULL, rel INTEGER NOT NULL, PRIMARY KEY(node, dk, other, rel)) WITHOUT ROWID`);
  }

  seed(lo, hi, n) {
    const sql = this.ctx.storage.sql;
    const props = new Uint8Array(105).fill(7);
    const labels = new Uint8Array([1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
    this.ctx.storage.transactionSync(() => {
      for (let i = lo; i <= hi; i++) {
        sql.exec("INSERT OR REPLACE INTO node(id, labels, props) VALUES (?, ?, ?)", i, labels, props);
        for (let k = 0; k < 3; k++) {
          const to = 1 + ((i * 7919 + k * 104729) % n);
          const rel = (i - 1) * 3 + k + 1;
          sql.exec("INSERT OR IGNORE INTO adj VALUES (?, 14, ?, ?)", i, to, rel);
          sql.exec("INSERT OR IGNORE INTO adj VALUES (?, 15, ?, ?)", to, i, rel);
        }
      }
    });
  }

  probe(kind, k, n) {
    const sql = this.ctx.storage.sql;
    let acc = 0, rowsRead = 0;
    let x = 12345;
    for (let j = 0; j < k; j++) {
      x = (x * 1103515245 + 12345) % 2147483648;
      const id = 1 + (x % n);
      if (kind === "point") {
        const c = sql.exec("SELECT labels, props FROM node WHERE id = ?", id);
        for (const row of c.raw()) acc += row[1].byteLength;
        rowsRead += c.rowsRead;
      } else if (kind === "adj") {
        const c = sql.exec("SELECT other, rel FROM adj WHERE node = ? AND dk = 14 ORDER BY other, rel", id);
        for (const row of c.raw()) acc += row[0];
        rowsRead += c.rowsRead;
      } else if (kind === "scan") {
        const c = sql.exec("SELECT id, props FROM node WHERE id > ? ORDER BY id LIMIT 256", id % (n - 256));
        for (const row of c.raw()) acc += row[1].byteLength;
        rowsRead += c.rowsRead;
      }
    }
    return { acc, rowsRead };
  }

  async fetch(request) {
    const url = new URL(request.url);
    const q = (name) => Number(url.searchParams.get(name));
    if (url.pathname === "/seed") {
      this.seed(q("lo"), q("hi"), q("n"));
      return Response.json({ ok: true });
    }
    if (url.pathname === "/probe") {
      return Response.json(this.probe(url.searchParams.get("kind"), q("k"), q("n")));
    }
    return new Response("not found", { status: 404 });
  }
}

export default {
  fetch(request, env) {
    return env.GRAPH.get(env.GRAPH.idFromName("probe")).fetch(request);
  },
};
