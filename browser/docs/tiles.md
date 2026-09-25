# City base map and samples

Build date: **2026-09-23**. Latest published daily build when prepared:
<https://build.protomaps.com/20260923.pmtiles> (Protomaps tile schema 4.15.2),
verified from <https://build-metadata.protomaps.dev/builds.json>.

The explorer's map and globe read one archive, `cities.pmtiles`, which covers
eight cities at street level: Calgary, New York, San Francisco, London, Rome,
Addis Ababa, Tokyo and Sydney. `browser/scripts/cities.mjs` lists them, each
with a city-sized box of about 30 × 30 km (Calgary's is the original
32 × 41 km box); the tile build and the sample generator both read that list.

Nothing is uploaded by the build. Ava owns the R2 upload and CORS configuration.

```sh
# From the repository root, using go-pmtiles 1.31.2. The build writes the
# extraction region (one rectangle per city, a GeoJSON MultiPolygon) beside the
# output as cities-region.geojson, then runs
#   pmtiles extract https://build.protomaps.com/20260923.pmtiles \
#     .tmp/tiles/cities.pmtiles --region=.tmp/cities-region.geojson --maxzoom=15
# and pmtiles verify, fetches the glyphs and sprites, and writes the exact
# upload manifest:
cd browser
node scripts/prepare-tiles.mjs ../.tmp/bin/pmtiles ../.tmp/tiles
# The Cities and Calgary samples, from that archive and OpenFlights:
node scripts/cities-sample.mjs ../.tmp/tiles/cities.pmtiles ../.tmp/openflights
```

`cities.pmtiles`: **356,022,111 bytes**, SHA-256
`f38c7346f8c1ab8940850b2295f54d5c1245730b620d55461af4b1afe157bca2`.
The maxzoom is 15. `pmtiles verify` passes. The low zooms hold only the tiles
over the eight boxes; the globe draws Natural Earth land below zoom 5 and the
basemap from zoom 5, so the rest of the world is never asked for. Glyphs and
sprites come from
[Protomaps' published assets](https://github.com/protomaps/basemaps-assets/tree/028c18f713baecad011301ff7a69acc39bcc2ae7),
pinned at `028c18f713baecad011301ff7a69acc39bcc2ae7`.

Upload keys, relative to the `zega-tiles` bucket root:

- `cities.pmtiles` → `https://tiles.zega.dev/cities.pmtiles`
- `fonts/OFL.txt`, plus `fonts/{fontstack}/{n}-{n+255}.pbf`, where `n` is
  every multiple of 256 from 0 through 65280 and `fontstack` is each of
  `Noto Sans Regular`, `Noto Sans Medium`, `Noto Sans Italic`, and
  `Noto Sans Devanagari Regular v1` (1,024 glyph files). These map to
  `https://tiles.zega.dev/fonts/{fontstack}/{range}.pbf`; preserve spaces in
  directory names (the browser percent-encodes them).
- `sprites/v4/light.json`, `light.png`, `light@2x.json`, `light@2x.png`
- `sprites/v4/dark.json`, `dark.png`, `dark@2x.json`, `dark@2x.png`

The upload contains 1,034 objects: the archive above, 1,025 font/license files
(17,715,972 bytes), and 8 sprite files (103,743 bytes), totalling
373,841,826 bytes. The fonts and sprites are byte-identical to the earlier
Calgary upload, so only `cities.pmtiles` is new. The full object list, byte
lengths and SHA-256 hashes are in the local `upload-manifest.json`. Do not
upload the region file, the assets tarball or build logs. Serve PMTiles with
byte-range support and `application/octet-stream`, glyphs as
`application/x-protobuf`, PNGs as `image/png`, and sprite JSON as
`application/json`. Enable CORS for zega sites and local explorer origins,
including range requests and exposed `Content-Range`/`ETag` headers. No keys
are used in the UI.

The previous archive, `calgary.pmtiles` (40,847,297 bytes, SHA-256
`6e6c64f5caf2c9cd1195370a54077282963b3492b735aaa53dd568155ae67782`, the
Calgary box alone), stays in the bucket until the explorer that reads
`cities.pmtiles` is deployed, so a page loaded before the switch keeps its
tiles. After that nothing reads it.

The style is built by `@protomaps/basemaps`' layer generator in `map-style.js`,
whose `BASEMAP` is the archive URL for both the map and the globe.
`theme.js` supplies the shared ground, water, rule, strong-rule, ink and soft tokens.
Sprites use the matching published light/dark flavor. Fonts use the published
Noto Sans stacks (including the Devanagari fallback) and retain `fonts/OFL.txt`.

## Samples

`scripts/cities-sample.mjs` writes both map samples from the archive. No
coordinate is authored: a place is an OSM POI label point from the archive's
z15 `pois` layer (a polygon POI uses Protomaps' label point), and a city is
the label point of its OSM locality in the z9 `places` layer. A place's name is
its OSM name, or its English name where the OSM name is in a non-Latin script
and has one (Tokyo, Addis Ababa).

- **Cities** (`cities.zql`, `cities-*.csv`, 9 KB): the eight cities, twelve
  places in each (the 3 parks, 3 museums, 4 sights and 2 cafés nearest the
  city's anchor POI in `cities.mjs`, no name used twice), and the 17 non-stop
  routes between them. A city flies to another when OpenFlights, at the commit
  pinned in `scripts/openflights.mjs`, has a non-stop route between an airport
  of each: one whose city is the city's name, in the city's country. Each route
  is stored once, from the city with fewer routes to the one with more (then by
  name), so a city's routes are its `route` and `inbound` ones together.
  Its display opens the globe over the North Atlantic with the routes animated;
  Calgary, New York, San Francisco, London, Rome and Addis Ababa face the
  reader, and Tokyo's routes arch over the pole. Sydney is on the far side
  from any view that shows London. The example bar holds every route, then one
  query per city that returns the city and its places, which the map view
  frames on its own.
- **Calgary** (`calgary.zql`, `calgary.csv`): the 30 places nearest the
  Calgary Tower (10 parks, 6 museums, 10 sights, 4 cafés) and a 1.5 km radius
  query, unchanged from the Calgary-only archive: the same tiles give the same
  places.

OSM data is licensed under **ODbL 1.0**. The basemap is an ODbL Produced Work;
`© OpenStreetMap contributors` links to <https://www.openstreetmap.org/copyright>
and is always visible, including tile failure. The generated samples are
OSM-derived databases under ODbL: their coordinates are extracted label points,
not surveyed entrances. The Cities routes are OpenFlights data (ODbL 1.0), and
the map credits OpenFlights while that sample is loaded. User nodes remain
separate from the basemap source.

## Test fixture

The committed 202,798-byte `tests/fixtures/cities.pmtiles` holds two zoom-14
tiles of the same archive: central London (tile 14/8186/5447) and central
Tokyo (14/14551/6452), retaining the same OSM attribution/license. The Cities
tests zoom into both and check that streets are drawn. It is only a test
fixture. Tests intercept requests locally; no test requires external tiles.
