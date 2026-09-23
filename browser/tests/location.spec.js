import { test, expect } from './offline.js';

test('WASM spatial queries equal a seeded 10000-point scan and survive snapshot restore', async ({ page }) => {
  await page.goto('/');
  const report = await page.evaluate(async () => {
    const { default: init, ZegaWasm } = await import('/pkg/zega_wasm.js');
    await init();
    const db = new ZegaWasm();
    const schema = 'type Place { n: Int at: Point from (lat, lon) }';
    let seed = 0x517cc1b7;
    const random = () => { seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5; return (seed >>> 0) / 4294967296; };
    const points = Array.from({ length: 10000 }, (_, n) => {
      const a = random(), b = random();
      const lat = n % 10 === 0 ? 89 + a : n % 10 === 1 ? -89 - a : a * 180 - 90;
      const lon = n % 10 === 2 ? 179 + b : n % 10 === 3 ? -179 - b : b * 360 - 180;
      return { n, lat, lon };
    });
    db.run_with_sources(schema, 'mutation json ["points.json"] { Place(n: $n) { id } }', JSON.stringify({ 'points.json': JSON.stringify(points) }));
    const radians = (x) => x * Math.PI / 180;
    const distance = (a, b) => {
      const h = Math.min(1, Math.max(0, Math.sin(radians(a.lat - b.lat) / 2) ** 2 + Math.cos(radians(a.lat)) * Math.cos(radians(b.lat)) * Math.sin(radians(a.lon - b.lon) / 2) ** 2));
      return 2 * 6371008.8 * Math.atan2(Math.sqrt(h), Math.sqrt(1 - h));
    };
    const errors = [];
    const compare = (source, expected) => {
      const actual = JSON.parse(db.run(schema, source)).map((row) => row.n);
      if (JSON.stringify(actual) !== JSON.stringify(expected)) errors.push(source);
    };
    let maxError = 0;
    for (const origin of [{ lat: 0, lon: 179.99 }, { lat: 0, lon: -179.99 }, { lat: 89.999, lon: 30 }, { lat: -89.999, lon: -100 }]) {
      const p = `point(${origin.lat}, ${origin.lon})`;
      const measured = points.map((row) => ({ ...row, distance: distance(origin, row) }));
      for (const radius of [0, 1500, 100000, 2000000, 21000000]) {
        compare(`{ Place(distance(at, ${p}) <= ${radius}) { n } }`, measured.filter((row) => row.distance <= radius).map((row) => row.n));
      }
      const nearest = [...measured].sort((a, b) => a.distance - b.distance || a.n - b.n);
      for (const k of [0, 1, 17, 10001]) compare(`{ Place order by distance(at, ${p}) limit ${k} { n } }`, nearest.slice(0, k).map((row) => row.n));
      const projection = JSON.parse(db.run(schema, `{ Place { n distance(at, ${p}) } }`));
      for (const row of projection) maxError = Math.max(maxError, Math.abs(row.distance - measured[row.n].distance));
    }
    for (const [south, west, north, east] of [[-10, 179, 10, -179], [89.5, -180, 90, 180], [-90, 170, -89.5, -170], [-90, -180, 90, 180]]) {
      compare(`{ Place(within_box(at, point(${south}, ${west}), point(${north}, ${east}))) { n } }`, points.filter((p) => p.lat >= south && p.lat <= north && (west <= east ? p.lon >= west && p.lon <= east : p.lon >= west || p.lon <= east)).map((p) => p.n));
    }
    const query = '{ Place order by distance(at, point(0, 180)) limit 5 { n at } }';
    const before = db.run(schema, query);
    const restored = new ZegaWasm();
    restored.import_base64(db.export_base64());
    if (restored.run(schema, query) !== before) errors.push('snapshot restore');
    restored.run(schema, 'mutation { Place(n: 0) set at: point(0, 180) { at } }');
    const near = '{ Place(distance(at, point(0, 180)) <= 0) { n } }';
    if (JSON.parse(restored.run(schema, near))[0]?.n !== 0) errors.push('update index');
    restored.delete_node(1);
    if (JSON.parse(restored.run(schema, near)).length !== 0) errors.push('delete index');
    restored.free();
    db.free();
    return { errors, maxError };
  });
  expect(report.errors).toEqual([]);
  expect(report.maxError).toBeLessThan(0.002);
});
