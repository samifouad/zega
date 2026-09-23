import init, { ZegaWasm } from './wasm/zega_wasm.js';

export { ZegaWasm };

let ready;

/** Initialize the shared WASM module and create an independent in-memory database. */
export async function createDatabase(options = {}) {
  ready ??= init({
    module_or_path: options.wasm ?? new URL('./wasm/zega_wasm_bg.wasm', import.meta.url),
  }).catch(error => {
    ready = undefined;
    throw error;
  });
  await ready;
  return new ZegaWasm();
}
