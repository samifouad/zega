/* tslint:disable */
/* eslint-disable */

export class ZegaWasm {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Run a v2 schema-language query. `schema` is the text of `schema.zql`.
     * `source` is one read or one `mutation`.
     * Run a `.zql` file of `schema`, `unique`, `mutation`, and `query` blocks.
     */
    apply(source: string): string;
    /**
     * Parse and type-check. Returns a JSON array of diagnostics. An empty
     * array means the schema and query are clean.
     */
    check(schema: string, source: string): string;
    /**
     * `field` is the relationship name on the source node's type.
     */
    connect(schema: string, from_id: number, field: string, to_id: number): void;
    delete_node(id: number): void;
    delete_relationship(id: number): void;
    /**
     * Serialize the whole graph database to a base64 string, so the
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
    constructor();
    query(zql: string, params_json: string): string;
    run(schema: string, source: string): string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_zegawasm_free: (a: number, b: number) => void;
    readonly zegawasm_apply: (a: number, b: number, c: number) => [number, number, number, number];
    readonly zegawasm_check: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly zegawasm_connect: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number];
    readonly zegawasm_delete_node: (a: number, b: number) => [number, number];
    readonly zegawasm_delete_relationship: (a: number, b: number) => [number, number];
    readonly zegawasm_export_base64: (a: number) => [number, number, number, number];
    readonly zegawasm_graph: (a: number) => [number, number, number, number];
    readonly zegawasm_import_base64: (a: number, b: number, c: number) => [number, number];
    readonly zegawasm_new: () => [number, number, number];
    readonly zegawasm_query: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly zegawasm_run: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
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
