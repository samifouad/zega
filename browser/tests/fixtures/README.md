# City fixture

`cities.pmtiles` (202,798 bytes) holds two zoom-14 tiles of the 2026-09-23
cities extract (`../../docs/tiles.md`): central London (14/8186/5447) and
central Tokyo (14/14551/6452). Source: https://build.protomaps.com/20260923.pmtiles.
© OpenStreetMap contributors, ODbL 1.0; https://www.openstreetmap.org/copyright.

Rebuild from the repository root, with a region of one tiny box at the centre
of each tile (`.tmp/fixture-region.geojson`: a MultiPolygon of ±0.0003° boxes
around -0.120850,51.515580 and 139.735107,35.666222):

```sh
.tmp/bin/pmtiles extract .tmp/tiles/cities.pmtiles browser/tests/fixtures/cities.pmtiles \
  --region=.tmp/fixture-region.geojson --minzoom=14 --maxzoom=14
```

Playwright's routes serve range requests from this local file and block all
external network access. Monaco's pre-existing CDN requests are served from the
pinned `monaco-editor` dev dependency by `../offline.js`.

`sprites/` and the three Latin font ranges in `fonts/` are test copies of the
published assets at `protomaps/basemaps-assets@028c18f713baecad011301ff7a69acc39bcc2ae7`.
Noto's OFL is in `fonts/OFL.txt`; sprite design licenses are in
`../../vendor/basemaps/LICENSE`.
