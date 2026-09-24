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
    apply_with_sources(source: string, sources: string): string;
    /**
     * Return a diagnostic report with rendered text and editor source spans.
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
    load_locations(source: string, document: boolean): string;
    constructor();
    /**
     * Nodes a ZQL `*path` search has expanded since this database opened.
     */
    nodes_expanded(): number;
    /**
     * Preview metadata and cells come from the same Rust parser as insertion.
     */
    preview_import(text: string): string;
    query(zql: string, params_json: string): string;
    /**
     * Rows a ZQL filter has been tested on since this database was created.
     * An `index { }` block lowers it; results never change.
     */
    rows_examined(): number;
    run(schema: string, source: string): string;
    /**
     * Raw source texts keyed by the locations written in ZQL. JS only transports
     * bytes; JSON/CSV parsing, binding and insertion stay in the engine.
     */
    run_with_sources(schema: string, source: string, sources: string): string;
    /**
     * Return checked schema types and the explicit display configuration as JSON.
     */
    schema(source: string): string;
    /**
     * PCA and full-vector explanations, attached to a query result by the host.
     */
    vector_view(schema: string, result: string, kind: string, selected: number | null | undefined, k: number, threshold: number): string;
}

/**
 * Format a ZQL file or editor pane using the canonical Rust formatter.
 */
export function format(source: string): string;

/**
 * The canonical byte-faithful JSON layout (APS 12).
 */
export function format_json(source: string): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_zegawasm_free: (a: number, b: number) => void;
    readonly format: (a: number, b: number) => [number, number, number, number];
    readonly format_json: (a: number, b: number) => [number, number];
    readonly zegawasm_apply: (a: number, b: number, c: number) => [number, number, number, number];
    readonly zegawasm_apply_with_sources: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly zegawasm_check: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly zegawasm_connect: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number];
    readonly zegawasm_delete_node: (a: number, b: number) => [number, number];
    readonly zegawasm_delete_relationship: (a: number, b: number) => [number, number];
    readonly zegawasm_export_base64: (a: number) => [number, number, number, number];
    readonly zegawasm_graph: (a: number) => [number, number, number, number];
    readonly zegawasm_import_base64: (a: number, b: number, c: number) => [number, number];
    readonly zegawasm_load_locations: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly zegawasm_new: () => [number, number, number];
    readonly zegawasm_nodes_expanded: (a: number) => [number, number, number];
    readonly zegawasm_preview_import: (a: number, b: number, c: number) => [number, number, number, number];
    readonly zegawasm_query: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly zegawasm_rows_examined: (a: number) => [number, number, number];
    readonly zegawasm_run: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly zegawasm_run_with_sources: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number, number];
    readonly zegawasm_schema: (a: number, b: number, c: number) => [number, number, number, number];
    readonly zegawasm_vector_view: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number, number];
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
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
