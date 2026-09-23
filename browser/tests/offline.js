import { test as base, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { resolve, sep } from 'node:path';

export const test = base.extend({
  offlineAssets: [async ({ context }, use) => {
    const root = resolve('node_modules/monaco-editor/min');
    await context.route('**/*', async (route) => {
      const url = new URL(route.request().url());
      if (url.hostname === '127.0.0.1' || url.hostname === 'localhost') return route.continue();
      const prefix = '/npm/monaco-editor@0.52.2/min/';
      if (url.hostname === 'cdn.jsdelivr.net' && url.pathname.startsWith(prefix)) {
        const path = resolve(root, decodeURIComponent(url.pathname.slice(prefix.length)));
        if (!path.startsWith(root + sep)) return route.abort();
        const contentType = path.endsWith('.js') ? 'text/javascript' : path.endsWith('.css') ? 'text/css' : 'application/octet-stream';
        return route.fulfill({ body: await readFile(path), contentType });
      }
      return route.abort();
    });
    await use();
  }, { auto: true }],
});
export { expect };
