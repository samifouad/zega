import type { InitInput, ZegaWasm } from './wasm/zega_wasm.js';

export { ZegaWasm } from './wasm/zega_wasm.js';
export type { InitInput } from './wasm/zega_wasm.js';

export interface DatabaseOptions {
  /** Override the WASM source on the first call (for custom asset hosting/bundlers). */
  wasm?: InitInput | Promise<InitInput>;
}

/** Initialize once, then create a fresh database. Call db.free() when finished. */
export function createDatabase(options?: DatabaseOptions): Promise<ZegaWasm>;
