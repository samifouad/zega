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
Both APIs reject invalid displays using the normal schema diagnostics. The server
has no schema endpoint; its routes are `/health`, `/zql`, `/vector-view` and
`/graph`.

Table sections use stored nodes and declared fields, with sortable values and
relationship chips. Graph type filters also filter relationship endpoints. Maps
plot coordinates projected by the current query; include `@id lat lon` to preserve
identity even when two nodes share the same values. Clicking a map marker, table
cell/chip, or graph node opens the same inspector. The Calgary button loads the
committed OSM sample and selects its declared default; the Cities button loads
eight cities, their places and the routes between them on the globe
([tiles.md](tiles.md#samples)); the Flights button loads
the OpenFlights sample (below). Light/dark theme preference
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

## Globe (APS 9)

```zql
schema {
  type Country {
    name: String
    iso: String<iso2>
  }

  type City {
    name: String
    at: Point
  }

  display {
    globe(@zoom: 1.4, @tilt: 20, @center: @point(51.05, -114.07)) { Country } : Default
    map { City }
  }
}
```

[APS 9](https://github.com/zegadb/aps/issues/9) adds a `globe` view. It uses
MapLibre's globe projection, the same stack as `map`, and becomes the flat
(Mercator) map as you zoom in: the sphere blends into Mercator between zoom 10
and 12.

Settings follow the view name. Every setting is optional:

- `@zoom`: a number from 0 to 22 (default 1.5). At or below 3 it is as far in
  as the view goes; see [framing](#framing-a-tilted-globe).
- `@tilt`: degrees from straight down, 0 to 85 (default 0).
- `@center`: `@point(latitude, longitude)` (default `@point(20, 0)`).

The checker fills in the defaults, so `Zega::schema` and `ZegaWasm.schema`
always return a complete camera on the globe view, e.g.
`{ "kind": "globe", "types": ["Country"], "globe": { "zoom": 1.4, "tilt": 20.0, "center": { "lat": 51.05, "lon": -114.07 } } }`.
Other views have no settings; `map(@zoom: 2)` is an error at the settings.
Unknown or repeated settings, out-of-range numbers, a `@center` that is not a
`@point(...)`, and a bare `point(...)` are diagnosed at their source spans.

Each listed type needs a country code or coordinates: a `String<iso2>` field,
a `Point` field, or the `lat: Float` + `lon: Float` pair. Without braces, at
least one schema type must have one.

`String<iso2>` is a unit-typed string, like `String<url>`. It stores one of the
249 assigned ISO 3166-1 alpha-2 codes, in capitals (`CA`, `GB`, `JP`). Writes,
updates, CSV/JSON imports and relationship fields reject anything else,
including lowercase codes and user-assigned codes such as `XK`. Optional
fields may be null. Filters and text/range indexes work as they do on `String`.

The globe draws the stored nodes of its listed types. Country outlines whose
code matches a node's first `String<iso2>` field are highlighted in the theme's
accent colour. Clicking one opens that node in the shared inspector. `Point`
fields (or `lat`/`lon`) plot as markers, which are also clickable. Water, land,
borders and highlights follow the explorer's light and dark theme. The OSM
basemap appears from zoom 5, as the globe flattens, and a highlighted
country's fill fades out between zoom 5 and 9, so a city's streets are not
drawn under the accent wash; its outline stays. If it fails, the countries
and places still draw on plain ground. If the outlines fail, the places still
draw and a notice says so. Terrain and relief are not part of this phase.

Country outlines are Natural Earth's 1:110m Admin 0 countries (v5.1.2,
public domain), bundled with the explorer as `browser/data/countries-110m.geojson`.
The file is 197,623 bytes (66,258 gzipped). It has 177 outlines, and each keeps
only `iso`, `name` and `label` (below), with coordinates rounded to three
decimals. `browser/scripts/prepare-countries.mjs` rebuilds the file from the
pinned source and checks the source's SHA-256 first. `iso` comes from Natural
Earth's `ISO_A2_EH`, which fills the gaps for France and Norway. Kosovo (`XK`,
not an ISO assignment), Northern Cyprus and Somaliland have no code, so they
are never highlighted. The bundled file is a static asset, so the globe needs
no tile server for its outlines.

### Relationships as arcs

The globe draws the relationships among its nodes as arcs that lift off the
sphere, the way a flight map shows routes. A relationship is drawn when both
of its ends have a location: a place at its `Point` (or `lat`/`lon`), a
country at its label point. The label point is precomputed by
`prepare-countries.mjs` as the pole of inaccessibility of the country's
largest polygon, so it sits inside the main landmass (a centroid would put
Norway, Chile or the United States in the sea). The relationships are the
same ones the graph view draws as edges: those in the stored graph whose ends
are both among the view's types.

Each arc follows the great circle from source to target and rises
`lift · (0.25 + 0.75 · d/π)` globe radii at its middle, where `d` is the
route's angular length, so long routes rise higher and short ones still lift.
Below about 900 km the rise is capped at `2 · lift · d`, so a hop across a
city arches over it instead of climbing hundreds of kilometres above the
camera. The arc is rendered on the GPU through a MapLibre custom layer: one
instanced draw for every arc, with the sphere drawn depth-only first so arcs
behind the planet are hidden by depth and an arc rising over the limb shows
where it clears the surface. As the globe becomes the flat map (zoom 10 to
12) the arcs follow MapLibre's own blend, lifted by the same height in
mercator altitude, and are drawn on the neighbouring world copies so a route
across the antimeridian stays whole. The great circle is computed once per
arc as an orthonormal basis, so an antipodal pair takes the same route every
frame.

Animated edges move their dashes from source to target at 20 px/s and send a
pulse along the route every four seconds; static edges are solid. Clicking an
arc opens the relationship in the shared inspector: its kind, both ends by
name, then its properties. Arcs take the theme's accent colour.

The gear under the zoom buttons opens the globe's settings, kept in
`localStorage` under `zega.browser.globe` like the graph's layout settings:

- **Lift**: the arc height, 0 to 0.5 of the globe radius (default 0.18).
- **Edges**: `animated` or `static`. The default is `static` when the
  reader prefers reduced motion, otherwise `animated`.
- **Auto-spin**: turns the globe slowly eastward, slowing as you zoom in and
  stopping while you drag or once the map is flat.

`scripts/bench-arcs.mjs` measures frame times with 500 and 5,000 animated
arcs headless; the target is 60 fps with 5,000.

### The Flights sample

The **Flights** button loads a flight map: the busiest airport of the 45
busiest countries, the 15 busiest airports overall and YYC (52 airports in 45
countries), with every non-stop route among them (715). "Busiest" is the
number of distinct non-stop routes in the whole dataset; a route counts when
both ends are IATA-coded airports and it has no stops.

The data is OpenFlights' airports, routes and countries, made available under
the [ODbL 1.0](https://openflights.org/data.php) with their contents under the
DbCL 1.0. The three CSV files in `browser/samples/` are a derived database
under the same licence, and the map credits OpenFlights in its attribution
while the sample is loaded. `browser/scripts/prepare-flights.mjs` rebuilds the
files from `github.com/jpatokal/openflights` at commit
`7d1a611e070295dba776d6afb86e57d0d1aa1cef` and refuses any other source:

| file | SHA-256 |
|---|---|
| `data/airports.dat` | `9387cdb38df5bd664da823f8ccb69fdd9b33a1888f5b7cca09c34a3cd9ff59f9` |
| `data/routes.dat` | `bd373706238134f619c624c606dccc74c05c2582a977c489c81de501735f2390` |
| `data/countries.dat` | `5cbd1a7da0f4f8003f595d22d80f025f483caad7a0672354ef9bc70221d348ed` |

The sample is 5 KB gzipped.

Each route is stored once, from the smaller airport to the larger hub
("busiest" is the number of distinct non-stop routes in the whole dataset), so
Calgary's routes are its `route`s and Amsterdam's are its `inbound` ones. The
schema declares both ends of the same `ROUTE` relationship on `Airport`, and
`BASE` between an airport and its `Country` (`String<iso2>`).

Its display opens the globe by default, tilted 40° and centred on the North
Atlantic at 55°N, so the Europe–North America corridor faces the reader with its arcs
rising, with the arcs animated unless the reader prefers reduced motion. The
example bar holds four queries: routes out of Calgary, routes into Amsterdam,
Canada's airports with their routes, and the five airports nearest the Calgary
Tower. Which sample is loaded persists with the panes, so the bar and the
credit come back on reload and go with the data on clear.

### Query focus

The globe draws the stored graph, with the query's relationships in focus.
A result object that names a stored node (by `@id`, or by every selected
property agreeing), with a relationship field under it whose objects name
nodes too, follows those relationships; the globe draws them at full strength
and animated, and every other stored relationship faint and still, as
context. When the result follows none, all draw alike. So "Out of Calgary"
lights up Calgary's ten routes over the rest of the network, and the count
reads `10 of 715 relationships`. Running a query changes the output pane and
the focus, and leaves the globe where the reader put it; a change to the
stored graph, the camera, the theme or the credit redraws it.

A dense set, more than 300 arcs, draws thinner ribbons (1.1 px instead of
1.6) with a lighter ink (55%), and its out-of-focus arcs fainter still (10%,
against 20%), so hundreds of routes read as a network rather than a band along
the limb.

### Framing a tilted globe

A globe view at or below zoom 3 is a view of the planet, and the view shows
the whole planet in the pane: `@zoom` is then as far in as the view goes, and
a pane too small for the planet at that zoom zooms out until the planet fits
with a 14 px margin, and back in as the pane grows, never past the schema's
zoom and never over a zoom the reader chose. Above zoom 3 the camera is a
region's and is exactly the schema's. MapLibre keeps the
map's centre point at the centre of the pane, so a tilted globe hangs below
it; the view measures where the planet's centre lands in the arc layer's frame
and pads the map below by twice the offset, which puts the planet itself in
the middle. The silhouette radius is measured from the same frame (the circle
at `acos(R/D)` from the camera direction, with `R/D` read off clip-space `w`).
On the flat map there is no planet to frame and the padding is removed. The
sample's `@zoom: 1.0` fills the explorer's pane at 1440 px; at 390 px the view
zooms out to fit.

### Without WebGL2

MapLibre draws with WebGL2 only. When a browser cannot provide it, the globe
and the map say so in the view ("WebGL2 unavailable. This browser cannot draw
the globe."), as they do for a failed basemap, and the query still answers in
the output pane. The arc layer frees its shaders as soon as their programs are
linked, so switching views does not accumulate them.
