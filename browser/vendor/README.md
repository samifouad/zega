# Vendored map renderer

Runtime files are copied from exact npm versions in `package-lock.json`:

| Directory | Package | Version | License |
| --- | --- | --- | --- |
| maplibre-gl | maplibre-gl | 6.11.1 | BSD-3-Clause |
| pmtiles | pmtiles | 4.5.0 | BSD-3-Clause |
| basemaps | @protomaps/basemaps | 5.7.2 | BSD-3-Clause, MIT, CC0 visual design |

Run `npm ci && node scripts/vendor-maps.mjs` in `browser/` to refresh the runtime
files. PMTiles uses its standalone distribution (including fflate), with an ES
module export for `Protocol` and `PMTiles` (the latter for small same-origin extracts read whole, e.g. `data/monaco.pmtiles`). Source-map references are removed because source maps are not shipped.
MapLibre's main, shared and worker ES modules are all local. No map JavaScript or
CSS is loaded from a CDN. License texts accompany each package; Protomaps license
sources were pinned at PMTiles `aec8fa1341222fdddb3318e9ffa8e18e19b312f7` and
basemaps `42ffaaa4a85a41bfcb23e43cc0f5b492a5eca123`.
