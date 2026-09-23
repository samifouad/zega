import { mkdir, readFile, writeFile } from 'node:fs/promises';
const copies = {
  'maplibre-gl/dist': ['maplibre-gl.mjs', 'maplibre-gl-shared.mjs', 'maplibre-gl-worker.mjs', 'maplibre-gl.css'],
  '@protomaps/basemaps/dist/esm': ['index.js'],
};
for (const [source, files] of Object.entries(copies)) {
  const name = source.startsWith('@') ? 'basemaps' : source.split('/')[0];
  await mkdir(`vendor/${name}`, { recursive: true });
  for (const file of files) {
    let contents = await readFile(`node_modules/${source}/${file}`, 'utf8');
    contents = contents.replace(/^\/\/# sourceMappingURL=.*$/gm, '');
    await writeFile(`vendor/${name}/${file}`, contents);
  }
}
await mkdir('vendor/pmtiles', { recursive: true });
const pmtiles = await readFile('node_modules/pmtiles/dist/pmtiles.js', 'utf8');
await writeFile('vendor/pmtiles/index.js', pmtiles.replace(/^\/\/# sourceMappingURL=.*$/gm, '') + '\nexport const Protocol = pmtiles.Protocol;\n');
// NPM distributions omit some licenses; source licenses are pinned in vendor/README.md.
