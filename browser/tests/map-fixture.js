import { readFile } from 'node:fs/promises';

export async function tileFixture(page) {
  const archive = await readFile('tests/fixtures/calgary.pmtiles');
  await page.route('https://tiles.zega.dev/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname !== '/calgary.pmtiles') {
      try { return route.fulfill({ body: await readFile(`tests/fixtures${decodeURIComponent(url.pathname)}`), contentType: url.pathname.endsWith('.json') ? 'application/json' : url.pathname.endsWith('.png') ? 'image/png' : 'application/x-protobuf' }); }
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
