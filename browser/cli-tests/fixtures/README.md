# Calgary fixture

`calgary.pmtiles` (68,132 bytes) is the zoom-0 world tile of the 2026-09-23
Calgary Protomaps extract, byte-identical to the fixture this test used
before the explorer's basemap moved to the shared, multi-city
`../../tests/fixtures/cities.pmtiles` (`../../tests/fixtures/README.md`).
That shared fixture only holds tiles for the two spots its own Cities-sample
tests draw (London, Tokyo); the "embedded Calgary Point map" test in
`../explorer.spec.js` queries around Calgary instead, so it needs a basemap
archive covering that instead — declaring zoom 0 as both its min and max
zoom lets MapLibre overzoom this one world tile at any camera zoom, same as
before. Source: https://build.protomaps.com/20260923.pmtiles.
© OpenStreetMap contributors, ODbL 1.0; https://www.openstreetmap.org/copyright.

Rebuild from the repository root:

```sh
.tmp/bin/pmtiles extract .tmp/tiles/calgary.pmtiles browser/cli-tests/fixtures/calgary.pmtiles --maxzoom=0
```

(`.tmp/tiles/calgary.pmtiles` is the full-pyramid Calgary extract that
`browser/scripts/prepare-tiles.mjs` used to build before the multi-city
archive replaced it — `pmtiles extract <build> .tmp/tiles/calgary.pmtiles
--bbox=-114.32,50.84,-113.86,51.21 --maxzoom=15`.)

Fonts and sprites for this test still come from `../../tests/fixtures`
(`map-fixture.js`'s `tileFixture` serves those from `root`, only the archive
itself from `archive`), so nothing here duplicates those assets.
