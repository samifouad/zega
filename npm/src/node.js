import { readFile } from 'node:fs/promises';
import init, { ZegaWasm } from './wasm/zega_wasm.js';

export { ZegaWasm };

let ready;

/** Node fetch cannot load file: URLs; read the same bundled WASM as bytes. */
export async function createDatabase(options = {}) {
  ready ??= (async () => {
    const wasm = options.wasm ?? await readFile(new URL('./wasm/zega_wasm_bg.wasm', import.meta.url));
    return init({ module_or_path: wasm });
  })().catch(error => {
    ready = undefined;
    throw error;
  });
  await ready;
  return new ZegaWasm();
}
