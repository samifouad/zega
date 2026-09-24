import { test, expect } from './offline.js';

// The same road net and expected JSON as zega/tests/path.rs, so the browser
// build must return byte-identical routes.
const ROADS = `schema {
  type Junction { name: String at: Point road -> Junction[] { km: Float<km> } }
}
unique { Junction { name } }
mutation { Junction(name: "A" && at: point(51.0, -114.0)) { name } }
mutation { Junction(name: "B" && at: point(51.0, -113.99)) { name } }
mutation { Junction(name: "C" && at: point(51.0, -113.98)) { name } }
mutation { Junction(name: "D" && at: point(51.01, -113.99)) { name } }
mutation { Junction(name: "A") { road -> link Junction(name: "B") { &km: 0.8 } } }
mutation { Junction(name: "B") { road -> link Junction(name: "C") { &km: 0.8 } } }
mutation { Junction(name: "A") { road -> link Junction(name: "D") { &km: 1.4 } } }
mutation { Junction(name: "D") { road -> link Junction(name: "C") { &km: 1.4 } } }
mutation { Junction(name: "A") { road -> link Junction(name: "C") { &km: 2.5 } } }
`;

const FEWEST = '{"name":"A","road":{"cost":1,"edges":[{"from":1,"id":5,"props":{"km":2.5},"to":3,"type":"road"}],"hops":1,"nodes":[{"hops":0,"km":null,"name":"A"},{"hops":1,"km":2.5,"name":"C"}]}}';
const CHEAPEST = '{"name":"A","road":{"cost":1.6,"edges":[{"from":1,"id":1,"props":{"km":0.8},"to":2,"type":"road"},{"from":2,"id":2,"props":{"km":0.8},"to":3,"type":"road"}],"hops":2,"nodes":[{"hops":0,"km":null,"name":"A"},{"hops":1,"km":0.8,"name":"B"},{"hops":2,"km":0.8,"name":"C"}]}}';

test('WASM path finding returns the native routes and A* expands fewer nodes', async ({ page }) => {
  await page.goto('/');
  const report = await page.evaluate(async ({ ROADS }) => {
    const { default: init, ZegaWasm } = await import('/pkg/zega_wasm.js');
    await init();
    const run = (query) => {
      const db = new ZegaWasm();
      try {
        return JSON.stringify(JSON.parse(db.apply(`${ROADS}query { ${query} }`)));
      } finally {
        db.free();
      }
    };
    const routes = {
      fewest: run('Junction(name: "A") { name road *path -> Junction(name: "C") { name &km &hops } }'),
      dijkstra: run('Junction(name: "A") { name road *path by &km -> Junction(name: "C") { name &km &hops } }'),
      astar: run('Junction(name: "A") { name road *path by &km toward at -> Junction(name: "C") { name &km &hops } }'),
      unreachable: run('Junction(name: "C") { road *path by &km -> Junction(name: "A") { name } }'),
    };

    // A 30 x 30 grid about 100 m apart; each road is its straight line in metres.
    const schema = 'schema { type Junction { n: Int at: Point road -> Junction[] { m: Int<m> } } }\nunique { Junction { n } }';
    const side = 30;
    const db = new ZegaWasm();
    const points = [];
    for (let i = 0; i < side * side; i++) points.push([51 + Math.floor(i / side) * 0.0009, -114 + (i % side) * 0.0014]);
    const nodes = points.map(([lat, lon], n) => ({ n, at: { lat, lon } }));
    db.run_with_sources(schema, 'mutation json ["nodes.json"] { Junction(n: $n) { n } }', JSON.stringify({ 'nodes.json': JSON.stringify(nodes) }));
    const metres = (a, b) => {
      const rad = (d) => (d * Math.PI) / 180;
      const h = Math.sin(rad(b[0] - a[0]) / 2) ** 2 + Math.cos(rad(a[0])) * Math.cos(rad(b[0])) * Math.sin(rad(b[1] - a[1]) / 2) ** 2;
      return 2 * 6371008.8 * Math.asin(Math.sqrt(h));
    };
    const roads = [];
    for (let i = 0; i < side * side; i++) {
      const [row, col] = [Math.floor(i / side), i % side];
      for (const j of [col + 1 < side ? i + 1 : -1, row + 1 < side ? i + side : -1]) {
        if (j < 0) continue;
        const m = Math.ceil(metres(points[i], points[j]));
        roads.push({ from: i, to: j, m }, { from: j, to: i, m });
      }
    }
    db.run_with_sources(schema, 'mutation json ["roads.json"] { Junction(n: $from) { road -> link Junction(n: $to) { &m: $m } } }', JSON.stringify({ 'roads.json': JSON.stringify(roads) }));
    const start = 15 * side;
    const goal = start + 22;
    const measure = (how) => {
      const before = db.nodes_expanded();
      const result = JSON.parse(db.run(schema, `{ Junction(n: ${start}) { road *path${how} -> Junction(n: ${goal}) { n } } }`));
      return { expanded: db.nodes_expanded() - before, cost: result.road.cost, hops: result.road.hops };
    };
    const dijkstra = measure(' by &m');
    const astar = measure(' by &m toward at');
    const diagnostic = JSON.parse(db.check(schema, '{ Junction { road *path toward at -> Junction { n } } }')).diagnostics[0]?.message;
    db.free();
    return { routes, dijkstra, astar, diagnostic };
  }, { ROADS });
  expect(report.routes.fewest).toBe(FEWEST);
  expect(report.routes.dijkstra).toBe(CHEAPEST);
  expect(report.routes.astar).toBe(CHEAPEST);
  expect(report.routes.unreachable).toBe('{"road":null}');
  expect(report.astar.cost).toBe(report.dijkstra.cost);
  expect(report.astar.hops).toBe(22);
  expect(report.dijkstra.expanded).toBeGreaterThan(400);
  expect(report.astar.expanded).toBe(22);
  expect(report.diagnostic).toBe('toward needs a weight measured in a distance');
});
