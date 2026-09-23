import { createDatabase, type DatabaseOptions, type ZegaWasm } from 'zega';
import init, { initSync, type InitInput } from 'zega/wasm';

const options: DatabaseOptions = {};
const db: ZegaWasm = await createDatabase(options);
const result: string = db.run('type Person { name: String }', '{ Person { name } }');
db.kv_set('ttl', '1', 1n);
db.free();
const input: InitInput = new Uint8Array();
void [init, initSync, input, result];
