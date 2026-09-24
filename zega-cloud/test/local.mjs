import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { localServer, root } from './local-server.mjs';
const schema = 'schema { type Item { key: String value: Int payload?: String } } unique { Item { key } }';
const insert = key => `mutation { Item(key: "${key}" && value: 1) { key value } }`;
const read = 'query { Item { key value } }';
let server;
async function post(graph, action, body = {}, headers = {}) {
  const response = await fetch(`${server.endpoint}/g/${graph}/${action}`, { method: 'POST', headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(body), signal: AbortSignal.timeout(30000) });
  let value;
  try { value = await response.json(); } catch { value = null; }
  return { response, value };
}
const zql = (graph, query, id = randomUUID(), extra = {}) => post(graph, 'zql', { schema, query }, { 'Request-Id': id, ...extra });
const control = (graph, action) => post(graph, `_spike-${action}`);
async function result(graph, query = read) { const r = await zql(graph, query); assert.equal(r.response.status, 200, JSON.stringify(r.value)); return r.value.result; }
async function stats(graph) { const r = await control(graph, 'stats'); assert.equal(r.response.status, 200, JSON.stringify(r.value)); return r.value.result; }
async function evict(graph) { try { await control(graph, 'evict'); } catch {} }

test('SQLite Durable Object integration', { timeout: 180000 }, async t => {
  await rm(resolve(root, '.tmp/local-integration'), { force: true, recursive: true });
  server = await localServer({ name: 'local-integration', port: 8799 });
  t.after(async () => server.stop());
  await t.test('native JSON envelope; graph isolation; Request-Id required', async () => {
    const missing = await post('one', 'zql', { schema, query: insert('missing') });
    assert.equal(missing.response.status, 400);
    const r = await zql('one', insert('a'), 'roundtrip');
    assert.equal(r.response.status, 200, JSON.stringify(r.value));
    assert.equal(r.value.ok, true);
    assert.ok(r.response.headers.has('x-zega-heap-bytes'));
    const rows = await result('one');
    assert.deepEqual(rows, [{ key: 'a', value: 1 }]);
    assert.deepEqual(await result('two'), []);
    const nullable = await post('one', 'zql', { schema, query: read, sources: null });
    assert.equal(nullable.response.status, 200);
    assert.deepEqual(nullable.value.result, rows);
    const tooBig = await post('one', 'zql', { schema, query: 'x'.repeat(1048576) });
    assert.equal(tooBig.response.status, 413);
    assert.deepEqual(await result('one'), rows);
  });
  await t.test('eviction and SIGKILL/restart preserve acknowledged data and ids', async () => {
    const before = (await control('one', 'graph')).value;
    const ids = (await stats('one')).ids;
    await evict('one');
    const cold = await control('one', 'graph');
    assert.deepEqual(cold.value, before);
    assert.ok(cold.response.headers.has('x-zega-cold-start-ms'));
    assert.deepEqual((await stats('one')).ids, ids);
    await server.stop('SIGKILL');
    server = await localServer({ name: 'local-integration', port: 8799 });
    assert.deepEqual((await control('one', 'graph')).value, before);
  });
  await t.test('same Request-Id returns original result and writes no SQLite rows', async () => {
    const query = 'mutation { Item(key: "a") set value: 2 { key value } }';
    const first = await zql('one', query, 'increment-once');
    assert.equal(first.response.status, 200, JSON.stringify(first.value));
    await zql('one', 'mutation { Item(key: "a") set value: 3 { key value } }', 'later-write');
    const before = await stats('one');
    const retry = await zql('one', query, 'increment-once');
    assert.deepEqual(retry.value, first.value);
    assert.equal(retry.response.headers.get('x-zega-retry'), 'true');
    const after = await stats('one');
    assert.equal(after.walRows, before.walRows);
    assert.equal(after.walBytes, before.walBytes);
    assert.equal(after.requestRows, before.requestRows);
    assert.deepEqual(await result('one'), [{ key: 'a', value: 3 }]);
    await evict('one');
    assert.deepEqual((await zql('one', query, 'increment-once')).value, first.value);
    const conflict = await zql('one', insert('different'), 'increment-once');
    assert.equal(conflict.response.status, 409);
  });
  await t.test('fail after WAL INSERT and after engine success: rollback then abort; no phantom blocks', async () => {
    for (const fault of ['append', 'before-result']) {
      for (let attempt = 0; attempt < 5; attempt++) {
        const key = `${fault}-${attempt}`;
        const before = (await control('one', 'graph')).value;
        const oldStats = await stats('one');
        let failed;
        try { failed = await zql('one', insert(key), key, { 'X-Zega-Fault': fault }); } catch {}
        assert.ok(!failed || failed.response.status >= 500);
        assert.deepEqual((await control('one', 'graph')).value, before);
        const now = await stats('one');
        assert.equal(now.walRows, oldStats.walRows);
        assert.equal(now.requestRows, oldStats.requestRows);
        const committed = await zql('one', insert(key), key);
        assert.equal(committed.response.status, 200, JSON.stringify(committed.value));
      }
    }
  });
  await t.test('lost response after commit is resolved by retry without reapplying', async () => {
    const id = 'lost-ack';
    let dropped;
    try { dropped = await zql('lost-ack', insert('once'), id, { 'X-Zega-Fault': 'after-commit' }); } catch {}
    assert.ok(!dropped || dropped.response.status >= 500);
    assert.deepEqual(await result('lost-ack'), [{ key: 'once', value: 1 }]);
    const before = await stats('lost-ack');
    const retry = await zql('lost-ack', insert('once'), id);
    assert.equal(retry.response.status, 200, JSON.stringify(retry.value));
    assert.equal(retry.response.headers.get('x-zega-retry'), 'true');
    assert.equal((await stats('lost-ack')).walRows, before.walRows);
  });
  await t.test('separate blocks commit separately; failed block has no partial rows', async () => {
    const query = `${schema}\nmutation { Item(key: "good" && value: 1) { key } }\nmutation json ["./block.json"] { Item(key: $key && value: $value) { key } }`;
    const r = await post('blocks', 'zql', { query, document: true, sources: { './block.json': '[{"key":"bad","value":1},{"key":"good","value":2}]' } }, { 'Request-Id': 'partial-doc' });
    assert.equal(r.response.status, 400);
    assert.equal(r.response.headers.get('x-zega-failed-block'), '2');
    await evict('blocks');
    assert.deepEqual(await result('blocks'), [{ key: 'good', value: 1 }]);
    const before = await stats('blocks');
    const retry = await post('blocks', 'zql', { query, document: true, sources: { './block.json': '[{"key":"bad","value":1},{"key":"good","value":2}]' } }, { 'Request-Id': 'partial-doc' });
    assert.deepEqual(retry.value, r.value);
    assert.equal((await stats('blocks')).walRows, before.walRows);
  });
  await t.test('snapshot chunks, rollback replacement, trim, cold replay and retry survive', async () => {
    const payload = 'p'.repeat(240000);
    for (let i = 0; i < 12; i++) {
      const r = await zql('snapshot', `mutation { Item(key: "${i}" && value: ${i} && payload: "${payload}") { key } }`, `chunk-${i}`);
      assert.equal(r.response.status, 200, JSON.stringify(r.value));
    }
    const before = await result('snapshot');
    const snap = await control('snapshot', 'snapshot');
    assert.equal(snap.response.status, 200, JSON.stringify(snap.value));
    assert.ok(snap.value.result.snapshotParts >= 3);
    assert.ok(snap.value.result.maxSnapshotPart <= 1048576);
    assert.equal(snap.value.result.walRows, 0);
    const newRow = await zql('snapshot', insert('after-snapshot'), 'after-snapshot');
    assert.equal(newRow.response.status, 200);
    const all = await result('snapshot');
    const failed = await control('snapshot', 'snapshot-fail');
    assert.equal(failed.response.status, 503);
    await evict('snapshot');
    assert.deepEqual(await result('snapshot'), all);
    assert.equal((await stats('snapshot')).walRows, 1);
    const old = await zql('snapshot', `mutation { Item(key: "0" && value: 0 && payload: "${payload}") { key } }`, 'chunk-0');
    assert.equal(old.response.headers.get('x-zega-retry'), 'true');
    assert.equal(all.length, before.length + 1);
  });
  await t.test('log size triggers checkpoint and trim automatically', async () => {
    const payload = 's'.repeat(300000);
    for (let i = 0; i < 30; i++) {
      const r = await zql('threshold', `mutation { Item(key: "${i}" && value: ${i} && payload: "${payload}") { key } }`, `threshold-${i}`);
      assert.equal(r.response.status, 200, JSON.stringify(r.value));
    }
    const before = await result('threshold');
    const stored = await stats('threshold');
    assert.ok(stored.snapshotBytes >= 8 * 1024 * 1024);
    assert.ok(stored.walRows < 30);
    await evict('threshold');
    assert.deepEqual(await result('threshold'), before);
  });
  await t.test('evicted wasm instance survives GC during 10000 later reads', async () => {
    for (let i = 0; i < 10000; i++) {
      const r = await post('threshold', 'zql', { schema, query: 'query { Item(key: "0") { key value } }' });
      assert.equal(r.response.status, 200, `read ${i}: ${JSON.stringify(r.value)}`);
      assert.deepEqual(r.value.result, { key: '0', value: 0 });
    }
  });
  await t.test('parallel retries apply exactly one mutation block', async () => {
    const replies = await Promise.all(Array.from({ length: 12 }, () => zql('concurrent', insert('once'), 'concurrent-id')));
    for (const r of replies) assert.equal(r.response.status, 200, JSON.stringify(r.value));
    assert.deepEqual(await result('concurrent'), [{ key: 'once', value: 1 }]);
    assert.equal((await stats('concurrent')).walRows, 1);
  });
});
