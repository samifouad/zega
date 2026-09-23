import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
const copies = {
  'maplibre-gl/dist': ['maplibre-gl.mjs', 'maplibre-gl-shared.mjs', 'maplibre-gl-worker.mjs', 'maplibre-gl.css'],
  'pmtiles/dist/esm': ['index.js'],
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
// NPM distributions omit some licenses; source licenses are pinned in vendor/README.md.
