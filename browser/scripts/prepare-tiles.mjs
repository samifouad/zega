// Build-only tool: never uploads or reads credentials.
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, readdir, writeFile, stat, copyFile } from 'node:fs/promises';
import { resolve, join, relative } from 'node:path';
import { createHash } from 'node:crypto';
import { region } from './cities.mjs';

const [pmtiles, outputArg] = process.argv.slice(2);
if (!pmtiles || !outputArg) throw new Error('Usage: node scripts/prepare-tiles.mjs /path/to/pmtiles /path/to/tiles');
const output = resolve(outputArg);
await mkdir(output, { recursive: true });
const buildDate = '20260923';
const source = `https://build.protomaps.com/${buildDate}.pmtiles`;
const assetsCommit = '028c18f713baecad011301ff7a69acc39bcc2ae7';
// One archive for every city in cities.mjs: a MultiPolygon region with one
// city-sized rectangle each. The region file sits beside the output, never in it.
const file = join(output, 'cities.pmtiles');
const regionFile = resolve(output, '../cities-region.geojson');
await writeFile(regionFile, JSON.stringify(region()) + '\n');
execFileSync(pmtiles, ['extract', source, file, `--region=${regionFile}`, '--maxzoom=15'], { stdio: 'inherit' });
execFileSync(pmtiles, ['verify', file], { stdio: 'inherit' });
const archive = join(output, '../basemaps-assets.tar.gz');
const response = await fetch(`https://github.com/protomaps/basemaps-assets/archive/${assetsCommit}.tar.gz`);
if (!response.ok) throw new Error(`Assets download: ${response.status}`);
await writeFile(archive, Buffer.from(await response.arrayBuffer()));
execFileSync('tar', ['-xzf', archive, '-C', resolve(output, '..')]);
const assets = resolve(output, `../basemaps-assets-${assetsCommit}`);
async function copyTree(from, to) {
  await mkdir(to, { recursive: true });
  for (const entry of await readdir(from, { withFileTypes: true })) {
    if (entry.isDirectory()) await copyTree(join(from, entry.name), join(to, entry.name));
    else await copyFile(join(from, entry.name), join(to, entry.name));
  }
}
await copyTree(join(assets, 'fonts'), join(output, 'fonts'));
await mkdir(join(output, 'sprites/v4'), { recursive: true });
for (const theme of ['light', 'dark']) for (const suffix of ['.json', '.png', '@2x.json', '@2x.png']) {
  await copyFile(join(assets, `sprites/v4/${theme}${suffix}`), join(output, `sprites/v4/${theme}${suffix}`));
}
const manifest = [];
async function walk(path) {
  if ((await stat(path)).isDirectory()) {
    for (const name of (await readdir(path)).sort()) await walk(join(path, name));
  } else {
    const bytes = await readFile(path);
    manifest.push({ key: relative(output, path).replaceAll('\\', '/'), bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
  }
}
for (const key of ['cities.pmtiles', 'fonts', 'sprites']) await walk(join(output, key));
await writeFile(join(output, 'upload-manifest.json'), JSON.stringify({ buildDate, source, assetsCommit, files: manifest }, null, 2) + '\n');
console.log(`${manifest.length} upload objects; ${manifest.reduce((n, f) => n + f.bytes, 0)} bytes. Exact keys and SHA-256: ${output}/upload-manifest.json`);
