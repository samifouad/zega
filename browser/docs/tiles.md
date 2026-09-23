# Calgary base map and sample

Build date: **2026-09-23**. Latest published daily build when prepared:
<https://build.protomaps.com/20260923.pmtiles> (Protomaps tile schema 4.15.2),
verified from <https://build-metadata.protomaps.dev/builds.json>.

The local output is `/Volumes/Projects/codex/zega-display/.tmp/tiles/`.
Nothing is uploaded by the build. Ava owns the R2 upload and CORS configuration.

```sh
# From the repository root, using go-pmtiles 1.31.2:
.tmp/bin/pmtiles extract https://build.protomaps.com/20260923.pmtiles \
  .tmp/tiles/calgary.pmtiles --bbox=-114.32,50.84,-113.86,51.21 --maxzoom=15
.tmp/bin/pmtiles verify .tmp/tiles/calgary.pmtiles
# Rebuild the archive and all assets, and write the exact upload manifest:
cd browser
node scripts/prepare-tiles.mjs ../.tmp/bin/pmtiles ../.tmp/tiles
node scripts/calgary-sample.mjs ../.tmp/tiles/calgary.pmtiles
```

`calgary.pmtiles`: **40,847,297 bytes**, SHA-256
`6e6c64f5caf2c9cd1195370a54077282963b3492b735aaa53dd568155ae67782`.
The maxzoom is 15. `pmtiles verify` passes. Glyphs and sprites come from
[Protomaps' published assets](https://github.com/protomaps/basemaps-assets/tree/028c18f713baecad011301ff7a69acc39bcc2ae7),
pinned at `028c18f713baecad011301ff7a69acc39bcc2ae7`.

Upload keys, relative to the `zega-tiles` bucket root:

- `calgary.pmtiles` → `https://tiles.zega.dev/calgary.pmtiles`
- Every file in `fonts/` → `https://tiles.zega.dev/fonts/{fontstack}/{range}.pbf`;
  preserve spaces in directory names (the browser percent-encodes them).
- `sprites/v4/light.json`, `light.png`, `light@2x.json`, `light@2x.png`
- `sprites/v4/dark.json`, `dark.png`, `dark@2x.json`, `dark@2x.png`

The upload contains 1,034 objects: the archive above, 1,025 font/license files
(17,715,972 bytes), and 8 sprite files (103,743 bytes), totalling
58,667,012 bytes. The full object list, byte lengths and SHA-256 hashes are in the local
`upload-manifest.json`. Do not upload the archive's intermediate tarball or build
logs. Serve PMTiles with byte-range support and `application/octet-stream`, glyphs
as `application/x-protobuf`, PNGs as `image/png`, and sprite JSON as
`application/json`. Enable CORS for zega sites and local explorer origins, including
range requests and exposed `Content-Range`/`ETag` headers. No keys are used in the UI.

The style is built by `@protomaps/basemaps`' layer generator in `map-style.js`.
`theme.js` supplies the shared ground, water, rule, strong-rule, ink and soft tokens.
Sprites use the matching published light/dark flavor. Fonts use the published
Noto Sans stacks (including the Devanagari fallback) and retain `fonts/OFL.txt`.

OSM data is licensed under **ODbL 1.0**. The basemap is an ODbL Produced Work;
`© OpenStreetMap contributors` links to <https://www.openstreetmap.org/copyright>
and is always visible, including tile failure. The generated Calgary sample is
an OSM-derived database under ODbL: its coordinates are extracted POI label points,
not surveyed entrances. User nodes remain separate from the basemap source.

The committed 68,132-byte `tests/fixtures/calgary.pmtiles` is the zoom-0 extraction
of the same archive, retaining the same OSM attribution/license. It is only a test
fixture. Tests intercept requests locally; no test requires external tiles.
