// Lifecycle glue only (copied from the zega-cloud spike): Rust answers every
// request; when it asks for eviction (x-zega-abort), abort the object after
// the Rust future has finished, so the next request is a cold start.
import { DurableObject } from "cloudflare:workers";
import { GraphObject as RustGraphObject } from "./build/index.js";
export { default } from "./build/index.js";

export class GraphObject extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.engine = new RustGraphObject(ctx, env);
  }
  async fetch(request) {
    const response = await this.engine.fetch(request);
    const reason = response.headers.get("x-zega-abort");
    if (reason) {
      this.engine = null;
      this.ctx.abort(reason);
    }
    return response;
  }
}
