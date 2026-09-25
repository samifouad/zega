# Graph components (APS 21)

Visual components made of surface × mark × encoding, each with a typed
contract. An agent picks one, writes ZQL whose result binds to the contract,
and sets its options: a **view spec**. The canvas draws specs on one shared
surface, notebook style. Demo: `/components/` in the explorer.

## Files

| file | what |
|---|---|
| `contract.schema.json` | JSON Schema for a component contract |
| `view-spec.schema.json` | JSON Schema for a view spec |
| `schema.js` | the JSON Schema subset interpreter used for both, and for option values |
| `validate.js` | `validate(spec, result, contract)`: shape, bindings against the real result, options; teaching errors |
| `canvas.js` | `createCanvas(element, { theme, historyLimit, store })` |
| `gear.js` | the canvas's settings panel, generated from the contract's descriptors |
| `storage.js` | history stores: `memoryStore()`, `localStorageStore(key)` |
| `registry.js` | component id → contract (eager) and code (lazy); surface kind → module (lazy) |
| `surfaces/map.js` | the street map with 3D buildings: one MapLibre instance |
| `path-on-map/` | the first component: `contract.json`, `index.js` |

## Canvas

```js
const canvas = createCanvas(element, { theme: 'light', store: localStorageStore('my.notebook') });
await canvas.ready;                         // history read back from the store
const entry = await canvas.load(spec, zqlResult, meta);  // throws ViewSpecError, leaving everything as it was
await canvas.restore(i);                    // redraw history[i] from its snapshot; no query
await canvas.undo();                        // restore the current entry's parent
await canvas.flush();                       // clear the drawing; the history stays
await canvas.setTheme('dark');
canvas.history;                             // [{ id, parent, time, spec, result, meta }], frozen
canvas.on(({ type, current, entry }) => …); // after load / restore / flush
```

An entry's `parent` is the entry on the canvas when it was made, so loading
after a restore branches. The surface, and its WebGL context, is created on
the first load and reused for every later one. MapLibre and component code
load on first use.

A store is `{ list(), put(entry), remove(ids), clear() }` over plain-JSON
entries; account sync and the desktop file system implement the same four.

## Contract

`roles` (data, bound from the result), `surfaceParams` and `options` (each a
JSON Schema with a `default`). Role types: `lonlat[]` (ZQL `Point` values or
`[lon, lat]`), `f64[]`, `f64`, `string`. See `path-on-map/contract.json`:

```json
"roles": {
  "path":  { "type": "lonlat[]", "required": true, "ordered": true, "minItems": 2, "unit": "degrees (WGS84)", "description": "…" },
  "value": { "type": "f64[]", "required": false, "sameLengthAs": "path", "description": "…" }
},
"options": {
  "lineWidth": { "type": "number", "minimum": 1, "maximum": 16, "default": 6, "description": "…" },
  "view": { "enum": ["2d", "3d"], "default": "3d", "description": "…" }
}
```

## View spec

```json
{ "component": "path-on-map", "version": "1",
  "binding": { "path": "points[].at", "value": "points[].speed", "label": "name" },
  "options": { "view": "3d", "bearing": -30 } }
```

A binding path reads a field (`name`), maps over a list (`points[].at`), or
maps over a list result (`[].at`).
