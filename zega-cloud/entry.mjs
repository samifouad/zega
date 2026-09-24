// Lifecycle glue only: routing, ZQL, transactions, SQLite and recovery are Rust.
// ctx.abort() cannot be caught, even by wasm-bindgen's `catch` binding. Calling
// it while a Rust future is being polled strands the shared async executor.
// Finish the Rust future and release the graph before invoking it from JS.
import { DurableObject } from 'cloudflare:workers';
import { GraphObject as RustGraphObject } from './build/index.js';
export { default } from './build/index.js';
export class GraphObject extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.engine = new RustGraphObject(ctx, env);
  }
  async fetch(request) {
    const response = await this.engine.fetch(request);
    const reason = response.headers.get('x-zega-abort');
    if (reason) {
      // The SDK exports a Proxy around the wasm-bindgen instance. Calling its
      // generated free() uses the Proxy as the FinalizationRegistry token,
      // leaving the underlying token registered (a later GC frees it twice).
      // Rust already dropped the graph; let wasm-bindgen own instance lifetime.
      this.engine = null;
      this.ctx.abort(reason);
    }
    return response;
  }
}
