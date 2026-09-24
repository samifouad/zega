import { readFile } from 'node:fs/promises';

// Serves the explorer's tile host from local files: the committed fixture by
// default, or another folder laid out like the upload (e.g. the full build).
export async function tileFixture(page, { root = 'tests/fixtures' } = {}) {
  const archive = await readFile(`${root}/cities.pmtiles`);
  await page.route('https://tiles.zega.dev/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname !== '/cities.pmtiles') {
      try { return route.fulfill({ body: await readFile(`${root}${decodeURIComponent(url.pathname)}`), contentType: url.pathname.endsWith('.json') ? 'application/json' : url.pathname.endsWith('.png') ? 'image/png' : 'application/x-protobuf' }); }
      catch { return route.fulfill({ status: 404 }); }
    }
    const range = /bytes=(\d+)-(\d+)/.exec(route.request().headers().range || '');
    if (!range) return route.fulfill({ body: archive, contentType: 'application/octet-stream' });
    const start = Number(range[1]), end = Math.min(Number(range[2]), archive.length - 1);
    return route.fulfill({ status: 206, body: archive.subarray(start, end + 1), headers: {
      'content-type': 'application/octet-stream', 'accept-ranges': 'bytes',
      'content-range': `bytes ${start}-${end}/${archive.length}`, 'access-control-allow-origin': '*',
    } });
  });
}
