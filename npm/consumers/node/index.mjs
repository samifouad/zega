import assert from 'node:assert/strict';
import { createDatabase, ZegaWasm } from 'zegadb';

const [db, empty] = await Promise.all([createDatabase(), createDatabase()]);
assert.ok(db instanceof ZegaWasm);
try {
  const result = JSON.parse(db.run('type Person { name: String }', 'mutation { Person(name: "Ada") { name } }'));
  assert.deepEqual(result, { name: 'Ada' });
  assert.deepEqual(JSON.parse(db.run('type Person { name: String }', '{ Person(name: "Ada") { name } }')), result);
  assert.deepEqual(JSON.parse(empty.run('type Person { name: String }', '{ Person(name: "Ada") { name } }')), null);
  const restored = await createDatabase();
  try {
    restored.import_base64(db.export_base64());
    assert.deepEqual(JSON.parse(restored.run('type Person { name: String }', '{ Person(name: "Ada") { name } }')), result);
  } finally { restored.free(); }
  console.log(`node: ${JSON.stringify(result)}`);
} finally {
  db.free();
  empty.free();
}
