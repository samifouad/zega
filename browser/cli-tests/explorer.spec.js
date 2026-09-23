import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';

async function start(directory) {
  const child = spawn(resolve('../.target/debug/zega'), ['explorer', '--port', '0', '--data', directory], { stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (data) => { stderr += data; });
  const url = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { child.kill(); reject(Error(`CLI startup timed out: ${stderr}`)); }, 15000);
    child.once('error', (error) => { clearTimeout(timeout); reject(error); });
    child.once('exit', (code) => { clearTimeout(timeout); reject(Error(`CLI exited ${code}: ${stderr}`)); });
    createInterface({ input: child.stdout }).once('line', (line) => { clearTimeout(timeout); resolve(line); });
  });
  return { url, async stop() { if (child.exitCode !== null) return; const exit = new Promise((resolve) => child.once('exit', resolve)); child.kill(); await exit; } };
}

test('embedded explorer writes through native ZQL and preserves data across reload and restart', async ({ page, request }) => {
  await mkdir('.tmp', { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(resolve('.tmp/cli-ui-'));
  let server = await start(directory);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const schema = 'type Player {\n  name: String\n  salary: Int\n}';
  const query = '{ Player { name salary } }';
  const read = async () => {
    const response = await request.post(`${server.url}/zql`, { data: { schema, query } });
    expect(response.ok()).toBe(true);
    return (await response.json()).result;
  };
  try {
    await page.goto(server.url);
    await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45000 });
    await expect(page.locator('.conn')).toContainText('native');
    for (const [name, mimeType, text, button] of [
      ['players.csv', 'text/csv', 'Name,Salary\n"Native, CSV",7\n', '#csv-import'],
      ['players.json', 'application/json', '[{"Name":"Native JSON","Salary":8}]', '#csv-merge'],
    ]) {
      await page.locator('#btn-csv').click();
      await page.locator('#csv-file').setInputFiles({ name, mimeType, buffer: Buffer.from(text) });
      await expect(page.locator('#csv-status')).toContainText('1 rows');
      await page.evaluate((schema) => window.monaco.editor.getEditors()
        .find((editor) => editor.getDomNode()?.closest('#csv-schema')).setValue(schema), schema);
      await page.locator(button).click();
      await expect.poll(async () => await page.locator('#csv-modal').isHidden() ? 'closed' : await page.evaluate(() => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest('#output')).getValue())).toBe('closed');
    }
    const expected = [{ name: 'Native, CSV', salary: 7 }, { name: 'Native JSON', salary: 8 }];
    expect(await read()).toEqual(expected);
    await page.reload();
    await expect(page.locator('#raw-count')).toContainText('2 nodes');
    expect(await read()).toEqual(expected);
    await server.stop();
    server = await start(directory);
    expect(await read()).toEqual(expected);
    await page.goto(server.url);
    await expect(page.locator('#raw-count')).toContainText('2 nodes');
    expect(await read()).toEqual(expected);
    expect(errors).toEqual([]);
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});
