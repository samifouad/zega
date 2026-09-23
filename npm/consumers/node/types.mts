import { createDatabase, type DatabaseOptions, type ZegaWasm } from 'zegadb';
import init, { initSync, type InitInput } from 'zegadb/wasm';

const options: DatabaseOptions = {};
const db: ZegaWasm = await createDatabase(options);
const result: string = db.run('type Person { name: String }', '{ Person { name } }');
db.free();
const input: InitInput = new Uint8Array();
void [init, initSync, input, result];
