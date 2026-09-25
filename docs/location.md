# Location

`Point` stores WGS84 latitude and longitude in degrees. Latitude is in
`[-90, 90]`, longitude in `[-180, 180]`; both must be finite numbers. Invalid
literals are type errors at the coordinate's span. `at?: Point` permits a
missing or null location.

```zql
schema {
  type Place {
    name: String
    at: Point
  }

  display {
    map { Place } : Default
    table
    graph
  }
}

mutation {
  Place(
    name: "Calgary Tower" &&
    at: @point(51.044299581787385, -114.06313508749008)
  ) { name at }
}
```

Point results are JSON objects, `{"lat": 51.044299581787385, "lon": -114.06313508749008}`.
The map accepts a Point field or the existing `lat: Float` + `lon: Float` pair.
Select the Point field (and preferably `@id`) to plot a query result; when
several Point fields are selected, the first in schema order supplies markers.
Optional locations that are absent have no marker. Views remain explicit in
`display {}`; coordinates never enable a view automatically.

## Queries

Radius filters use metres and include the boundary with `<=` (`<` excludes it):

```zql
query {
  Place(@distance(at, @point(51.044299581787385, -114.06313508749008)) <= 1500) {
    @id
    name
    at
    @distance(at, @point(51.044299581787385, -114.06313508749008))
  }
}
```

`@distance(...)` returns metres under the key `distance`; an alias such as
`metres: @distance(at, @point(51.0443, -114.0631))` also works. A missing optional
Point returns null and does not match spatial filters.

```zql
// Inclusive southwest and northeast corners.
query {
  Place(@within_box(at, @point(51.03, -114.09), @point(51.06, -114.04))) {
    @id
    name
    at
  }
}

// A west longitude greater than east crosses the antimeridian.
query {
  Place(@within_box(at, @point(-10, 179), @point(10, -179))) {
    @id
    name
    at
  }
}

// Nearest five, optionally combined with any existing filter.
query {
  Place order by @distance(at, @point(51.0443, -114.0631)) limit 5 {
    @id
    name
    at
    @distance(at, @point(51.0443, -114.0631))
  }
}
```

Spatial predicates combine with `&&` and `||`. `order by @distance(...)` sorts
nearest first, or farthest first with `desc`, breaking equal distances by node
ID, and a second key after a comma breaks them instead. Missing Points are excluded
from distance ordering. `limit` is a non-negative integer, after `order by`
when both occur, and before the selected fields. These clauses also work on
relationship selections. Ordering or limiting produces an array at the query
root, including when an equality filter is present. The clauses are read-only.

Distances use haversine on a sphere with mean Earth radius 6,371,008.8 metres,
not an ellipsoidal geodesic or a travel route. Box edges are inclusive numeric
coordinates: `[-180, 180]` covers all longitudes; `(180, -180)` covers both
representations of that meridian. South must not exceed north. Negative or
non-finite radius thresholds are errors.

## Loading

JSON objects may supply a Point directly under its field name. An explicit
binding also works:

```json
[
  {
    "name": "Calgary Tower",
    "at": { "lat": 51.044299581787385, "lon": -114.06313508749008 }
  }
]
```

```zql
mutation json ["./places.json"] {
  Place(name: $name && at: $at) { name at }
}
```

Separate columns require an explicit schema mapping. Column names refer to
source records, not additional fields on the stored type:

```zql
schema {
  type Place {
    name: String
    at: Point from (lat, lon)
  }
}

mutation csv ["./places.csv"] {
  Place(name: $name) { name at }
}
```

```csv
name,lat,lon
Calgary Tower,51.044299581787385,-114.06313508749008
```

Use `from (latitude, longitude)` for those column names, or any two numeric
source columns in latitude/longitude order. The same mapping works in JSON
loads. Without it, `lat`/`lon` columns are never guessed. An explicit template
assignment wins; otherwise a named Point object wins over mapped columns.
Missing or non-numeric mapped columns and invalid coordinates are load type
errors. All Point rows are bound and checked before the load writes any rows.
An optional Point can be absent when no mapping is declared, or explicitly
null; a declared mapping still requires both numeric source columns.

## Index and persistence

Every stored Point is automatically indexed by field name in a Morton
(Z-order) index over a fixed 16-bit grid per coordinate. Box and radius
filters seek conservative key intervals, then check the original coordinates
and exact haversine distance. Nearest-k expands indexed circles until enough
qualifying results are found. Boolean OR only prunes when both branches have
safe spatial candidates. Global queries may visit the entire index.

Insert, update, replacement and deletion maintain the index. WAL replay and
snapshot restore reconstruct it from the stored Point values, as they do the
label/property indexes. Existing value variant IDs are unchanged. All of this
runs in native and browser WASM without threads or network access; fetching
an import file or map tiles remains a host/UI concern.

The explorer's **Calgary** sample loads `samples/calgary.csv` through its
explicit mapping and opens the 1.5 km Tower query on the map.
