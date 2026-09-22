/* tslint:disable */
/* eslint-disable */

export class ZegaWasm {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Serialize the whole database (graph + KV) to a base64 string, so the
     * browser build can persist it across reloads.
     */
    export_base64(): string;
    /**
     * Every stored node and relationship, for the graph canvas.
     */
    graph(): string;
    /**
     * Restore a database previously produced by `export_base64`, replacing
     * current state.
     */
    import_base64(data: string): void;
    kv_del(key: string): boolean;
    kv_get(key: string): string;
    kv_set(key: string, value_json: string, ttl_secs?: bigint | null): void;
    constructor();
    query(zql: string, params_json: string): string;
    /**
     * Run a v2 schema-language query. `schema` is the text of `schema.zql`.
     * `source` is one read or one `mutation`.
     */
    run(schema: string, source: string): string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_zegawasm_free: (a: number, b: number) => void;
    readonly zegawasm_export_base64: (a: number) => [number, number, number, number];
    readonly zegawasm_graph: (a: number) => [number, number, number, number];
    readonly zegawasm_import_base64: (a: number, b: number, c: number) => [number, number];
    readonly zegawasm_kv_del: (a: number, b: number, c: number) => [number, number, number];
    readonly zegawasm_kv_get: (a: number, b: number, c: number) => [number, number, number, number];
    readonly zegawasm_kv_set: (a: number, b: number, c: number, d: number, e: number, f: number, g: bigint) => [number, number];
    readonly zegawasm_new: () => [number, number, number];
    readonly zegawasm_query: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly zegawasm_run: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
