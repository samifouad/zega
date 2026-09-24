# Schema display (APS 5)

[APS 5](https://github.com/zegadb/aps/issues/5) makes views an explicit schema contract:

```zql
schema {
  type Place {
    name: String
    lat: Float
    lon: Float
  }

  type Review { title: String }

  display {
    map { Place } : Default
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
{
  "views": [
    { "kind": "map", "types": ["Place"] },
    { "kind": "table", "types": ["Review"] },
    { "kind": "graph", "types": null }
  ],
  "default": "map"
}
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

## Node appearance and preview (APS 8)

```zql
schema {
  type Log {
    name: String
    notes: String
  }

  type Contract {
    name: String
    scan: String<url>
  }

  type Player {
    name: String
    photo?: String<url>
  }

  display {
    graph {
      Log(@shape: document)
      Contract(@shape: document, @image: &scan)
      Player(
        @shape: circle,
        @image: &photo,
        @size: 2
      )
    }
  }
}
```

Attributes belong to a type inside a display view. `@shape` is `circle` (the
default) or `document`; `@size` is `1` (default), `2`, or `3`; `@image` references
a field with `&`. Type entries can be separated by whitespace or commas.
Attributes never change stored data. Graph rendering uses the active graph
view's attributes; other views retain their own presentation.

`String<url>` stores an absolute HTTP(S) URL as an ordinary string. The scheme
is case-insensitive; a host is required and userinfo is forbidden. Writes,
updates, and bound CSV/JSON imports reject other schemes, malformed URLs and
whitespace. Optional URL fields may be null. String filters and indexes keep working. The same Rust
URL parser runs in native and WASM; URL validation does not fetch anything.
Images load only for visible nodes, over HTTP(S). Other schemes, absent images
and failed requests retain the default node appearance. Loading never changes
node geometry. Images use `referrerpolicy="no-referrer"`, with an HTML referrer
policy as a fallback. Plain image hosts do not need to opt into CORS.

Click a node to select it, then press Space for Quick Look. Keyboard users can
Tab to any node and press Space. The preview heading uses the schema's `name`
field, or `Type #id`; its two-column table includes declared properties and
relationships, with nested values rendered as readable text. Escape, Space,
the close button, or clicking outside closes it and restores focus to the node.
The native modal keeps focus inside and makes the background inert.

Implementation choices beyond [APS 8](https://github.com/zegadb/aps/issues/8):
size levels scale geometry by 1, 1.5 and 2; document pages start at 36×44 graph
units (circles retain radius 22). At graph zoom 2 or greater, visible documents
show the heading and first five rows with ellipses for long cells. Zooming out
restores the icon/image. Light-mode paper is white. Dark-mode paper is
`#cbd5e1` with `#334155` ink and a `#94a3b8` fold; miniature text meets AA
contrast. Edges intersect the actual circle or rounded, folded page outline.
Visible edge labels try the curve midpoint, then positions at 0.35, 0.65,
0.25 and 0.75, then bounded perpendicular offsets to avoid caption and node
boxes. Text metrics are cached, and placement is deterministic.
