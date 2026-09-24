import { test, expect } from './offline.js';

// zegadb/zega#46: formatting and running are two pipelines. Formatting keeps
// both ZQL panes formatted (on load, and 350ms after typing stops) and never
// runs. Running auto-runs 350ms after an edit, except while the query holds a
// mutation, and every run formats both panes first.

const find = (pane) => `window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#${pane}'))`;
const text = (page, pane) => page.evaluate(`${find(pane)}.getValue()`);
const output = (page) => text(page, 'output');
const before = (page, pane) => page.evaluate(`(() => { const e = ${find(pane)}; return e.getValue().slice(0, e.getModel().getOffsetAt(e.getPosition())); })()`);

async function ready(page) {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('50 nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
}

/** Puts `value` in a pane with the cursor at `offset` (default: the end) and focuses it. */
async function place(page, pane, value, offset = value.length) {
  await page.evaluate(({ pane, value, offset }) => {
    const editor = window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${pane}`));
    editor.setValue(value);
    editor.setPosition(editor.getModel().getPositionAt(offset));
    editor.focus();
  }, { pane, value, offset });
}

// Each pane: what is typed, where, and what it formats to. The query pane
// types into a mutation so auto-run stays paused: only the format pipeline
// can format it, never a run's format-first step.
const panes = {
  query: {
    start: 'mutation{Player(name: "Typed"){name}}', at: 35, typed: ' salary',
    typedText: 'mutation{Player(name: "Typed"){name salary}}',
    formatted: 'mutation {\n  Player(name: "Typed") { name salary }\n}\n',
    invalid: 'mutation{Player(name: "Typed"){name',
  },
  schema: {
    start: 'type Player{name: String}', at: 24, typed: ' salary: Int',
    typedText: 'type Player{name: String salary: Int}',
    formatted: 'type Player {\n  name: String\n  salary: Int\n}\n',
    invalid: 'type Player {name: String',
  },
};

test('both panes are formatted on load, including text restored from the last visit', async ({ page }) => {
  await ready(page);
  await expect(page.getByRole('button', { name: 'Format', exact: true })).toHaveCount(0);
  await page.evaluate(() => {
    localStorage.setItem('zega.v2.schema', 'type Player{name: String salary: Int}');
    localStorage.setItem('zega.v2.query', 'query{Player{name salary}}');
  });
  await page.reload();
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect.poll(() => text(page, 'schema')).toBe(panes.schema.formatted);
  await expect.poll(() => text(page, 'query')).toBe('query {\n  Player { name salary }\n}\n');
  // Load formatting is not an edit anyone made: there is nothing to undo.
  expect(await page.evaluate(`${find('query')}.getModel().canUndo()`)).toBe(false);
});

for (const [pane, c] of Object.entries(panes)) {
  test(`${pane}: typing with short pauses is never reformatted, and a typed space survives`, async ({ page }) => {
    await ready(page);
    await place(page, pane, c.start, c.at);
    await page.keyboard.type(c.typed, { delay: 120 });
    // Still the unformatted head: no format ran between keystrokes.
    expect(await text(page, pane)).toBe(c.typedText);
  });

  test(`${pane}: after a pause the text is formatted and the cursor stays after the same character`, async ({ page }) => {
    await ready(page);
    await place(page, pane, c.start, c.at);
    await page.keyboard.type(c.typed, { delay: 120 });
    await page.waitForTimeout(600);
    expect(await text(page, pane)).toBe(c.formatted);
    // Everything before the cursor, whitespace aside, is what was before it when typing stopped.
    const cursorAt = await before(page, pane);
    expect(cursorAt.replace(/\s+/g, '')).toBe((c.start.slice(0, c.at) + c.typed).replace(/\s+/g, ''));
    expect(cursorAt.at(-1)).toBe(c.typed.at(-1));
  });

  test(`${pane}: invalid text is never changed`, async ({ page }) => {
    await ready(page);
    await place(page, pane, c.invalid);
    await page.keyboard.type('x', { delay: 50 });
    await page.waitForTimeout(600);
    expect(await text(page, pane)).toBe(`${c.invalid}x`);
  });
}

test('a space just typed is not removed when typing stops on it', async ({ page }) => {
  await ready(page);
  await place(page, 'query', panes.query.start, panes.query.at);
  await page.keyboard.type(' ', { delay: 50 });
  await page.waitForTimeout(600);
  await page.keyboard.type('salary', { delay: 50 });
  await page.waitForTimeout(600);
  expect(await text(page, 'query')).toBe(panes.query.formatted);
});

test('Cmd+Z right after an auto-format restores the unformatted text in one step, and stays', async ({ page }) => {
  await ready(page);
  await place(page, 'query', panes.query.start, panes.query.at);
  await page.keyboard.type(panes.query.typed, { delay: 50 });
  await page.waitForTimeout(600);
  expect(await text(page, 'query')).toBe(panes.query.formatted);
  await page.keyboard.press('ControlOrMeta+z');
  expect(await text(page, 'query')).toBe(panes.query.typedText);
  await page.waitForTimeout(800); // past both pipelines' pause
  expect(await text(page, 'query')).toBe(panes.query.typedText);
  await page.keyboard.press('ControlOrMeta+z'); // then the typing
  expect(await text(page, 'query')).not.toContain('salary');
});

test('a query auto-runs after a 350ms pause', async ({ page }) => {
  await ready(page);
  await place(page, 'query', 'query { Player(name = "Connor McDavid") { name salary } }');
  await expect.poll(() => output(page), { timeout: 3000 }).toContain('12500000');
});

test('an edit to the schema re-runs the query', async ({ page }) => {
  await ready(page);
  await place(page, 'query', 'query { Player(name = "Connor McDavid") { name salary } }');
  await expect.poll(() => output(page), { timeout: 3000 }).toContain('12500000');
  await page.evaluate(`window.__runs = []; const db = window.__zega, run = db.run_with_sources.bind(db);
    db.run_with_sources = (...args) => { window.__runs.push(args[1]); return run(...args); };`);
  await page.evaluate(`(() => { const e = ${find('schema')}; e.setValue(e.getValue().replace('salary: Int', 'salary: Int // yearly')); })()`);
  await expect.poll(() => page.evaluate('window.__runs.length'), { timeout: 3000 }).toBe(1);
});

test('a mutation is formatted but not auto-run; the note shows; Run runs it; removing it resumes auto-run', async ({ page }) => {
  await ready(page);
  const note = page.locator('#autorun-note');
  await expect(note).toBeHidden();
  await place(page, 'query', 'mutation{Player(name: "Test Skater" && position: "C"){name}}');
  await expect(note).toBeVisible();
  await expect(note).toHaveText('auto-run paused: mutation');
  await page.waitForTimeout(800);
  expect(await text(page, 'query')).toBe('mutation {\n  Player(name: "Test Skater" && position: "C") { name }\n}\n');
  await expect(page.locator('#raw-count')).toContainText('50 nodes');
  await page.locator('#btn-run').click();
  await expect(page.locator('#raw-count')).toContainText('51 nodes');
  await place(page, 'query', 'query { Player(name = "Test Skater") { name position } }');
  await expect(note).toBeHidden();
  await expect.poll(() => output(page), { timeout: 3000 }).toContain('"position": "C"');
  await expect(page.locator('#raw-count')).toContainText('51 nodes');
});

test('Cmd/Ctrl+Enter runs a mutation too', async ({ page }) => {
  await ready(page);
  await place(page, 'query', 'mutation { Player(name: "Chord Skater" && position: "D") { name } }');
  await page.keyboard.press('ControlOrMeta+Enter');
  await expect(page.locator('#raw-count')).toContainText('51 nodes');
});

test('an explicit Run on unformatted text formats it first, and runs the formatted text', async ({ page }) => {
  await ready(page);
  await page.evaluate(`window.__runs = []; const db = window.__zega, run = db.run_with_sources.bind(db);
    db.run_with_sources = (...args) => { window.__runs.push(args[1]); return run(...args); };`);
  await place(page, 'query', 'query{Player(name = "Connor McDavid"){name salary}}');
  await page.locator('#btn-run').click(); // before either pipeline's pause ends
  const formatted = 'query {\n  Player(name = "Connor McDavid") { name salary }\n}\n';
  expect(await text(page, 'query')).toBe(formatted);
  await expect.poll(() => page.evaluate('window.__runs[0]')).toBe(formatted);
  await expect.poll(() => output(page)).toContain('12500000');
});
