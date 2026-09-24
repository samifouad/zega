import { readFile } from 'node:fs/promises';
import init, { format, format_json } from '../pkg/zega_wasm.js';
await init({ module_or_path: await readFile(new URL('../pkg/zega_wasm_bg.wasm', import.meta.url)) });
export { format, format_json };
