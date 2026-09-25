# Graph components (APS 21)

Visual components made of surface × mark × encoding, each with a typed
contract. An agent picks one, writes ZQL whose result binds to the contract,
and sets its options: a **view spec**. The canvas draws specs on one shared
surface, notebook style. Demo: `/components/` in the explorer.

## Files

| file | what |
|---|---|
| `schemas/v1/contract.schema.json` | a component contract, format 1 |
| `schemas/v1/view-spec.schema.json` | a view spec, format 1 |
| `schema.js` | the JSON Schema subset interpreter used for both, and for setting values |
| `validate.js` | `validate(spec, result, contract, rules)`, `applyFix(spec, fix)` |
| `migrate.js` | older view-spec formats → the current one; the breaking-change policy |
| `canvas.js` | `createCanvas(element, { theme, historyLimit, store, onError })` |
| `gear.js` | the canvas's settings panel, generated from the contract's settings |
| `storage.js` | history stores: `memoryStore()`, `localStorageStore(key)` |
| `registry.js` | component id → contract (eager) and code (lazy); surface kind → module (lazy) and rules; `checkContract` |
| `surfaces/map.js`, `surfaces/archives.js` | the street map with 3D buildings: one MapLibre instance |
| `path-on-map/` | the first component: `contract.json`, `index.js` |

## Canvas

```js
const canvas = createCanvas(element, { theme: 'light', store: localStorageStore('my.notebook') });
await canvas.ready;                            // stored entries read back; unusable ones dropped via onError
const entry = await canvas.load(spec, result); // throws ViewSpecError (or a draw error), leaving everything as it was
await canvas.restore(i);                       // redraw history[i] from its snapshot; no query
await canvas.undo();                           // restore the current entry's parent
await canvas.flush();                          // clear the drawing; the history stays
canvas.history;                                // [{ id, n, parent, time, spec, resultRef, meta }], frozen
canvas.resultOf(entry);                        // the result snapshot, stored once per resultRef
canvas.on(({ type, current, entry }) => …);    // load / restore / flush / sync (another tab)
```

One surface (one WebGL context) for the canvas's life; MapLibre and
component code load on first use. Two views of the same component in a row
update its sources and paint in place (`setData`, `setPaintProperty`) rather
than rebuilding layers. A draw that fails redraws the previous entry.

An entry's `parent` is the entry on the canvas when it was made, so loading
after a restore branches. Entry ids are random UUIDs; `n` is an ordinal for
display. A store is `{ list, getResult, put(entry, result), remove, clear,
watch? }`; results are stored once under the SHA-256 of their JSON. Account
sync and the desktop file system implement the same calls.

## Contract (format 1)

Data arrives as **rows**, optionally inside **groups**; each **role** reads
one field per `row`, per `group` or per `result`, typed in ZQL's value
vocabulary:

| type | value | normalised |
|---|---|---|
| `Point` | `{ lat, lon }` | `[lon, lat]` (on a map, lat within ±85.0511) |
| `Float`, `Int` | a finite number (`Int`: whole) | the number |
| `Bool`, `String` | | |
| `Enum` | a String from the role's `enum` | |
| `XY` | `{ x, y }` in the role's planar `frame` (rink, court, field), optionally `relativeTo` a surface parameter | `[x, y]` |

`shape` is `one` (default), `list` or `list<list>`. Roles may carry
`minimum`/`maximum` and a `unit` from a closed list: `ft m deg pct fraction
km/h yd s`. Surface parameters and options are **settings**: a JSON Schema
limited to exactly `type enum const minimum maximum minLength maxLength
pattern items prefixItems minItems maxItems format` (`format:
"lonlat-bounds"` = `[w, s, e, n]` with w < e, s < n), plus `default` and
`description`. Each contract ships 2–3 valid example specs and 1 invalid one
with its exact error.

path-on-map:

```json
"data":  { "groups": "optional", "minRows": 2 },
"roles": {
  "path":  { "per": "row",   "type": "Point",  "required": true },
  "value": { "per": "row",   "type": "Float",  "required": false },
  "label": { "per": "group", "type": "String", "required": false }
}
```

How the next components would declare themselves (not built):

```json
// globe arcs: one row per route
"data":  { "groups": "none" },
"roles": {
  "from":   { "per": "row", "type": "Point", "required": true },
  "to":     { "per": "row", "type": "Point", "required": true },
  "weight": { "per": "row", "type": "Float", "minimum": 0, "required": false }
}

// rink heat: one row per event, in rink feet
"roles": {
  "at":     { "per": "row", "type": "XY", "frame": "rink", "unit": "ft", "minimum": -100, "maximum": 100, "required": true },
  "weight": { "per": "row", "type": "Float", "minimum": 0, "required": false }
}

// route chart: groups are routes, rows their points, relative to the line of scrimmage
"data":  { "groups": "required" },
"roles": {
  "at":      { "per": "row",   "type": "XY", "frame": "field", "unit": "yd", "relativeTo": "los", "required": true },
  "outcome": { "per": "group", "type": "Enum", "enum": ["complete", "incomplete", "td"], "required": false }
},
"surfaceParams": { "los": { "type": "number", "minimum": 0, "maximum": 100, "default": 25, "description": "Line of scrimmage, yards from the own goal line." } }
```

## View spec (format 1)

```json
{ "format": 1, "component": "path-on-map", "version": "1",
  "binding": { "rows": "points", "path": "at", "value": "speed", "label": "name" },
  "options": { "view": "3d", "valueUnit": "km/h" } }
```

`groups` (from the result) and `rows` (from each group, or the result) are
paths to lists; `.` is the value itself. Role paths are fields, nested with
dots (at most 32 steps). Every error has `code`, `at`, `message`, `help` and,
where one exists, `fix` — a JSON Patch against the spec (`applyFix`).

## Versions

- Schemas live under `schemas/v<format>/` with matching `$id`s.
- A spec without `format` is format 0 (the first draft, list-path bindings)
  and is migrated (`migrate.js`) with a warning. Every shipped format keeps a
  migration.
- A component's major changes when an existing spec may stop binding; the
  registry then keeps the old major's contract and a migration. A minor adds;
  a spec written for a newer minor than the runtime has gets a warning.
- `ordered` was dropped: a JSON result does not say whether it was ordered.
  It returns when ZQL results carry ordering metadata.
