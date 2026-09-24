# Schema display (APS 5)

[APS 5](https://github.com/zegadb/aps/issues/5) makes views an explicit schema contract:

```zql
schema {
  type Place { name: String lat: Float lon: Float }
  type Review { title: String }
  display {
    map { Place }: Default
    table { Review }
    graph
  }
}
```

Entries retain declaration order. The default is the marked entry, or the first
entry when no mark is present. Without a display block the contract is graph only,
even when coordinates are present. Braces contain declared singular type names;
omitting braces includes all types. Empty blocks/lists, duplicate views/blocks,
unknown views/types, and multiple defaults are diagnosed at their source spans.

The checker requires `lat: Float` and `lon: Float` for maps. With a type list every
listed type must qualify; without one at least one schema type must qualify.
Timelines use the existing scalar conventions `year: Int` or `date: String`
(ISO date text), with the same per-type rule. Graph and table accept any type.
Optional coordinate/date fields remain optional data; absent values have no point
or timeline entry. Declaring a view never inspects stored data.

Rust's `Zega::schema(source)` returns the parsed `Schema`, including `display`.
Wasm's `ZegaWasm.schema(source)` returns the same JSON contract:

```json
{"views":[{"kind":"map","types":["Place"]},{"kind":"table","types":["Review"]},{"kind":"graph","types":null}],"default":"map"}
```

The JSON above is the `display` member. `types` also carries parsed fields,
relationship direction/targets and the checker's `timeline_field` for each type.
Both APIs reject invalid displays using the normal schema diagnostics. This base
revision's server exposes `/health` and `/cql`; it has no schema endpoint.

Table sections use stored nodes and declared fields, with sortable values and
relationship chips. Graph type filters also filter relationship endpoints. Maps
plot coordinates projected by the current query; include `@id lat lon` to preserve
identity even when two nodes share the same values. Clicking a map marker, table
cell/chip, or graph node opens the same inspector. The Calgary button loads the
committed OSM sample and selects its declared default. Light/dark theme preference
persists across reloads.

MapLibre, its worker modules/CSS, PMTiles and the Protomaps layer generator are
vendored. Map asset failure removes the basemap, preserves query markers, and
keeps the OSM attribution visible. The existing Monaco loader still uses its
existing CDN; tests serve pinned Monaco package files locally and block external
requests. An entirely offline explorer install must also provide Monaco locally.

`vector2d` and `vector3d` require a Vector field on every listed type (every
schema type when braces are omitted). They show the current query's selected
IDs, with engine-computed PCA positions, full-vector nearest neighbors and
relationship disagreement flags. Both use the shared inspector. The **Tickets**
button loads the deterministic synthetic support-ticket sample. See
[Vectors and meaning views](../../docs/vector.md) for syntax, metrics and controls.
