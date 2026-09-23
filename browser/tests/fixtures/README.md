# Calgary fixture

`calgary.pmtiles` (68,132 bytes) is the zoom-0 subset of the 2026-09-23 Calgary
Protomaps extract. Source: https://build.protomaps.com/20260923.pmtiles.
© OpenStreetMap contributors, ODbL 1.0; https://www.openstreetmap.org/copyright.

Rebuild from the repository root:

```sh
.tmp/bin/pmtiles extract .tmp/tiles/calgary.pmtiles browser/tests/fixtures/calgary.pmtiles --maxzoom=0
```

Playwright's routes serve range requests from this local file and block all
external network access. Monaco's pre-existing CDN requests are served from the
pinned `monaco-editor` dev dependency by `../offline.js`.

`sprites/` and the three Latin font ranges in `fonts/` are test copies of the
published assets at `protomaps/basemaps-assets@028c18f713baecad011301ff7a69acc39bcc2ae7`.
Noto's OFL is in `fonts/OFL.txt`; sprite design licenses are in
`../../vendor/basemaps/LICENSE`.
