import init, { ZegaWasm, format, format_json } from './pkg/zega_wasm.js';
import { renderGraph, stopSim } from './graph.js';
import { renderMap } from './map.js';
import { globeData, renderGlobe } from './globe.js';
import { renderVector } from './vector.js';
import { renderTable } from './table.js';
import { applyTheme } from './theme.js';
import { createEditors } from './editor.js';
import { openCsv, parseSchema } from './csv.js';
import { connectDatabase, RemoteDatabase } from './backend.js';
import { formatEditor, hasMutation, typingAfterSpace } from './zql-edit.js';

const LS_DB = 'zega.v2.since';
const LS_SCHEMA = 'zega.v2.schema';
const LS_QUERY = 'zega.v2.query';
const LS_SAMPLE = 'zega.v2.sample';

const mug = (id) => `https://assets.nhle.com/mugs/nhl/latest/${id}.png`;
const logo = (abbr) => `https://assets.nhle.com/logos/nhl/svg/${abbr}_light.svg`;
const flag = (code) => `https://flagcdn.com/w160/${code}.png`;

const SCHEMA = `type Team {
  name: String
  city: String
  logo: String
  playsFor -> Player[] { since?: Int }
}

type Player {
  name: String
  position: String
  face: String
  salary: Int
  playsFor <- Team
  born <- Country
}

type Country {
  name: String
  flag: String
  born -> Player[]
}`;

const QUERY = `{
  Country(name = "Canada") {
    name
    born -> Player {
      name
      salary
      playsFor <- Team { name }
    }
  }
}`;

const TOUR = [
  ['Canadian players', QUERY],
  ['Russia or Canada', `{
  Country(name = "Russia" || name = "Canada") {
    name
    born -> Player {
      name
      salary
      playsFor <- Team { name }
    }
  }
}`],
  ['Centers over $10M', `{
  Player(salary > 10000000 && position = "C") {
    name
    salary
    playsFor <- Team { name }
  }
}`],
  ['Oilers or Avalanche', `{
  Team(name = "Oilers" || name = "Avalanche") {
    name
    playsFor -> Player { name &since }
  }
}`],
  ['Golden Knights', `{
  Team(name = "Golden Knights") {
    name
    playsFor -> Player {
      name
      salary
      born <- Country { name }
    }
  }
}`],
  ['Germany', `{
  Country(name = "Germany") {
    name
    born -> Player {
      name
      salary
      playsFor <- Team { name }
    }
  }
}`],
  ['Hops from Canada', `{
  Country(name = "Canada") {
    born -> Player {
      name
      @hops
      playsFor <- Team { name @hops }
    }
  }
}`],
  ['Born outside Canada', `{
  Country(name != "Canada") {
    name
    born -> Player {
      name
      playsFor <- Team { name }
    }
  }
}`],
];

// The Flights sample (zega#83): the example bar while it is loaded. Routes
// are stored once, from the smaller airport to the larger hub, so Calgary's
// are its `route`s and Amsterdam's its `inbound` ones.
const CALGARY_TOWER = '@point(51.0443, -114.0631)';
const FLIGHTS_TOUR = [
  ['Out of Calgary', `{
  Airport(code: "YYC") {
    name
    city
    route -> Airport { code city }
  }
}`],
  ['Into Amsterdam', `{
  Airport(code: "AMS") {
    name
    inbound <- Airport {
      code
      city
      country -> Country { name }
    }
  }
}`],
  ["Canada's airports", `{
  Country(iso: "CA") {
    name
    airports <- Airport {
      code
      city
      route -> Airport { code }
    }
  }
}`],
  ['Nearest to the Calgary Tower', `{
  Airport order by @distance(at, ${CALGARY_TOWER}) limit 5 {
    code
    city
    @distance(at, ${CALGARY_TOWER})
  }
}`],
];
// The Cities sample: eight cities, the non-stop routes between them, and
// twelve OSM places in each. The first query focuses every route on the
// globe; each city's own query returns that city and its places, which the
// map view frames on its own.
const CITY_NAMES = ['Calgary', 'New York', 'San Francisco', 'London', 'Rome', 'Addis Ababa', 'Tokyo', 'Sydney'];
const CITIES_TOUR = [
  ['Every route', `{
  City {
    name
    country
    route -> City { name }
  }
}`],
  ...CITY_NAMES.map((name) => [name, `{
  City(name: "${name}") {
    name
    places <- Place {
      name
      kind
      at
    }
  }
}`]),
];
// Samples with an example bar and a data credit on the map. Which one is
// loaded persists with the panes, so both come back on reload and go with
// the data on clear.
const OPENFLIGHTS = '<a href="https://openflights.org/data.php" target="_blank" rel="noopener">OpenFlights</a> (ODbL)';
const SAMPLES = {
  flights: { tour: FLIGHTS_TOUR, credit: `Routes: ${OPENFLIGHTS}` },
  cities: { tour: CITIES_TOUR, credit: `Routes: ${OPENFLIGHTS}` },
};

function teamSeed(name, city, abbr, players) {
  const roster = players.map(([player, position, id, salary, since]) => {
    const joined = since == null ? '' : ` &since: ${since}`;
    return `playsFor -> Player(name: "${player}" && position: "${position}" && face: "${mug(id)}" && salary: ${salary}) { name salary${joined} }`;
  }).join('\n    ');
  return `mutation {
  Team(name: "${name}" && city: "${city}" && logo: "${logo(abbr)}") {
    name
    ${roster}
  }
}`;
}

function countrySeed(name, code, players) {
  const links = players.map((player) =>
    `born -> link Player(name: "${player}") { name }`
  ).join('\n    ');
  return [
    `mutation { Country(name: "${name}" && flag: "${flag(code)}") { name } }`,
    `mutation {
  Country(name: "${name}") {
    name
    ${links}
  }
}`,
  ];
}

const SEEDS = [
  teamSeed('Oilers', 'Edmonton', 'EDM', [
    ['Connor McDavid', 'C', 8478402, 12500000, 2015],
    ['Leon Draisaitl', 'C', 8477934, 14000000, 2014],
    ['Evan Bouchard', 'D', 8479999, 10500000, 2018],
    ['Zach Hyman', 'LW', 8475786, 5500000, 2021],
    ['Ryan Nugent-Hopkins', 'C', 8476454, 5125000, 2011],
    ['Mattias Ekholm', 'D', 8475218, 4000000, 2023],
  ]),
  teamSeed('Maple Leafs', 'Toronto', 'TOR', [
    ['Auston Matthews', 'C', 8479318, 13250000, 2016],
    ['William Nylander', 'RW', 8477939, 11500000, 2016],
    ['Morgan Rielly', 'D', 8476853, 7500000, 2013],
    ['John Tavares', 'C', 8475166, 4389280, 2018],
  ]),
  teamSeed('Golden Knights', 'Vegas', 'VGK', [
    ['Jack Eichel', 'C', 8478403, 13500000, 2021],
    ['Mitch Marner', 'RW', 8478483, 12000000, 2025],
    ['Mark Stone', 'RW', 8475913, 9500000, 2017],
  ]),
  teamSeed('Avalanche', 'Colorado', 'COL', [
    ['Nathan MacKinnon', 'C', 8477492, 12604000, 2013],
    ['Martin Necas', 'C', 8480039, 11500000, 2025],
    ['Cale Makar', 'D', 8480069, 9000000, 2019],
    ['Brock Nelson', 'C', 8475754, 7500000, 2025],
    ['Devon Toews', 'D', 8478038, 7250000, 2020],
    ['Gabriel Landeskog', 'LW', 8476455, 7000000, 2011],
    ['Nazem Kadri', 'C', 8475172, 5600000, 2022],
  ]),
  teamSeed('Penguins', 'Pittsburgh', 'PIT', [
    ['Erik Karlsson', 'D', 8474578, 11500000, 2023],
    ['Sidney Crosby', 'C', 8471675, 8700000, 2005],
    ['Kris Letang', 'D', 8471724, 6100000, 2006],
    ['Evgeni Malkin', 'C', 8471215, 5500000, 2006],
    ['Bryan Rust', 'RW', 8475810, 5125000, 2014],
  ]),
  teamSeed('Capitals', 'Washington', 'WSH', [
    ['Alex Tuch', 'RW', 8477949, 10500000, 2026],
    ['Pierre-Luc Dubois', 'C', 8479400, 8500000, 2024],
    ['Jordan Kyrou', 'RW', 8479385, 8125000],
    ['Tom Wilson', 'RW', 8476880, 6500000, 2013],
    ['Alex Ovechkin', 'LW', 8471214, 4250000, 2005],
  ]),
  teamSeed('Lightning', 'Tampa Bay', 'TBL', [
    ['Andrei Vasilevskiy', 'G', 8476883, 9500000, 2014],
    ['Nikita Kucherov', 'RW', 8476453, 9500000, 2013],
    ['Brayden Point', 'C', 8478010, 9500000, 2016],
    ['Jake Guentzel', 'LW', 8477404, 9000000, 2024],
    ['Victor Hedman', 'D', 8475167, 8000000, 2009],
    ['Brandon Hagel', 'LW', 8479542, 6500000, 2022],
    ['Anthony Cirelli', 'C', 8478519, 6250000, 2017],
  ]),
  ...countrySeed('Canada', 'ca', [
    'Connor McDavid', 'Evan Bouchard', 'Zach Hyman', 'Ryan Nugent-Hopkins',
    'Morgan Rielly', 'John Tavares', 'Mitch Marner', 'Mark Stone',
    'Nathan MacKinnon', 'Cale Makar', 'Devon Toews', 'Nazem Kadri',
    'Sidney Crosby', 'Kris Letang', 'Bryan Rust', 'Pierre-Luc Dubois',
    'Jordan Kyrou', 'Tom Wilson', 'Brayden Point', 'Brandon Hagel', 'Anthony Cirelli',
  ]),
  ...countrySeed('United States', 'us', [
    'Auston Matthews', 'Jack Eichel', 'Brock Nelson', 'Alex Tuch', 'Jake Guentzel',
  ]),
  ...countrySeed('Sweden', 'se', [
    'Mattias Ekholm', 'William Nylander', 'Gabriel Landeskog', 'Erik Karlsson', 'Victor Hedman',
  ]),
  ...countrySeed('Germany', 'de', ['Leon Draisaitl']),
  ...countrySeed('Czechia', 'cz', ['Martin Necas']),
  ...countrySeed('Russia', 'ru', [
    'Alex Ovechkin', 'Evgeni Malkin', 'Nikita Kucherov', 'Andrei Vasilevskiy',
  ]),
];

const $ = (sel) => document.querySelector(sel);
const graphEl = $('#graph');
let theme = localStorage.getItem('zega.theme') || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
if (!['light', 'dark'].includes(theme)) theme = 'light';
applyTheme(theme);
let activeView = null, displayKey = '', disposeView = null, globeKey = '';
let sampleKey = localStorage.getItem(LS_SAMPLE);
const sample = () => SAMPLES[sampleKey];
function setSample(key) {
  sampleKey = key;
  if (key) localStorage.setItem(LS_SAMPLE, key); else localStorage.removeItem(LS_SAMPLE);
}


const savedQuery = localStorage.getItem(LS_QUERY);
const editorsReady = createEditors({
  schema: localStorage.getItem(LS_SCHEMA) || SCHEMA,
  query: savedQuery || QUERY,
});

await init();
// The page's own database: wasm in the browser, or the CLI's native backend.
// `db` is what every operation uses; it becomes a RemoteDatabase while
// connected to a Zega Cloud graph, and `localDb` again on Disconnect.
const localDb = await connectDatabase(new ZegaWasm());
let db = localDb;
const LOCAL_LABEL = localDb.native ? 'local · native' : 'local · wasm';
$('#conn-label').textContent = LOCAL_LABEL;
const EMPTY_DB = localDb.native ? null : localDb.export_base64();
const saved = localDb.native ? null : localStorage.getItem(LS_DB);
if (saved) {
  try { localDb.import_base64(saved); } catch (e) { console.error(e); }
}
window.__zega = db;

const { schema: schemaEditor, query: queryEditor, output: outputEditor, raw: rawEditor, monaco } = await editorsReady;

function schemaText() { return schemaEditor.getValue(); }
function queryText() { return queryEditor.getValue(); }

// Text the page puts in a pane (a tour step, a sample, an import) arrives
// formatted and starts a fresh undo history; it is not an edit to react to.
let suppress = 0;
function setQuiet(editor, value) {
  suppress += 1;
  editor.setValue(value);
  formatPane(editor, { history: false });
  suppress -= 1;
}
function saveSources() {
  localStorage.setItem(LS_SCHEMA, schemaText());
  localStorage.setItem(LS_QUERY, queryText());
}

// Formatting is its own pipeline, apart from running (zegadb/zega#46). Each
// ZQL pane formats itself a pause after its last edit, when its text parses,
// and never runs anything. The CLI and both explorer backends use this same
// WASM formatter.
const FORMAT_DEBOUNCE_MS = 350;
const formatTimers = new Map();
const composing = new Set();
let selfFormatting = 0; // Our own edits are not someone typing.

function formatPane(editor, options) {
  clearTimeout(formatTimers.get(editor));
  selfFormatting += 1;
  try { return formatEditor(editor, format, options); } finally { selfFormatting -= 1; }
}

/**
 * A space or new line just typed at a focused cursor is where the next word
 * goes. Formatting would remove it, and the next word would join the last.
 */
function typingSpace(editor) {
  const model = editor.getModel();
  const position = editor.getPosition();
  return editor.hasTextFocus() && Boolean(position) && typingAfterSpace(model.getValue(), model.getOffsetAt(position));
}

/**
 * Formats both panes now, where they parse: on load, and before every run.
 * `spareTyping` leaves a pane alone while `typingSpace` holds; the auto-run
 * fires on the same pause as the auto-format and must not undo its care.
 */
function formatSources(options, { spareTyping = false } = {}) {
  for (const editor of [schemaEditor, queryEditor]) {
    if (!(spareTyping && typingSpace(editor))) formatPane(editor, options);
  }
  saveSources();
}

function scheduleFormat(editor) {
  clearTimeout(formatTimers.get(editor));
  formatTimers.set(editor, setTimeout(() => autoformat(editor), FORMAT_DEBOUNCE_MS));
}

function autoformat(editor) {
  // Wait out an IME composition or an open suggestion list rather than
  // rewrite the text under it.
  if (composing.has(editor) || editor.getDomNode()?.querySelector('.suggest-widget.visible')) {
    scheduleFormat(editor);
    return;
  }
  // Leave just-typed whitespace until the cursor moves on or the pane loses focus.
  if (typingSpace(editor)) return;
  if (formatPane(editor)) saveSources();
}

let formatTarget = queryEditor;
for (const editor of [schemaEditor, queryEditor]) {
  editor.onDidFocusEditorText(() => { formatTarget = editor; });
  editor.onDidBlurEditorText(() => scheduleFormat(editor));
  editor.onDidCompositionStart(() => composing.add(editor));
  editor.onDidCompositionEnd(() => { composing.delete(editor); scheduleFormat(editor); });
  // Undo and redo put back text on purpose; formatting it again would make
  // the undo impossible to keep. The next typed edit formats as usual.
  editor.onDidChangeModelContent((event) => {
    if (!selfFormatting && !suppress && !event.isUndoing && !event.isRedoing) scheduleFormat(editor);
  });
  editor.addAction({ id: 'zega.format', label: 'Format ZQL', contextMenuGroupId: '1_modification',
    keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS], run: () => { if (formatPane(editor)) saveSources(); } });
}
// Cmd/Ctrl+S formats now, and never opens the browser's save dialog.
document.addEventListener('keydown', event => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
    event.preventDefault();
    event.stopPropagation();
    if (formatPane(formatTarget)) saveSources();
  }
}, true);

monaco.editor.setTheme(theme === 'dark' ? 'vs-dark' : 'vs');
$('#btn-theme').textContent = theme === 'dark' ? 'Light' : 'Dark';
$('#btn-theme').onclick = () => {
  theme = theme === 'dark' ? 'light' : 'dark';
  localStorage.setItem('zega.theme', theme);
  applyTheme(theme);
  monaco.editor.setTheme(theme === 'dark' ? 'vs-dark' : 'vs');
  $('#btn-theme').textContent = theme === 'dark' ? 'Light' : 'Dark';
  resetView();
  drawGraph();
};


async function clearDatabase() {
  if (db.remote && !confirm(`Delete every node and relationship in the remote graph ${db.graphId}? This cannot be undone.`)) {
    throw new Error(`Nothing was deleted from ${db.graphId}.`);
  }
  setSample(null);
  if (db.native) await db.clear();
  else {
    db.import_base64(EMPTY_DB);
    try { localStorage.setItem(LS_DB, EMPTY_DB); } catch (e) { console.error(e); }
  }
  lastValue = null;
}

function persist() {
  localStorage.setItem(LS_SCHEMA, schemaText());
  localStorage.setItem(LS_QUERY, queryText());
  if (!db.native) { try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); } }
}

let lastValue = null;

function namesIn(value, into = new Set()) {
  if (!value || typeof value !== 'object') return into;
  if (Array.isArray(value)) {
    value.forEach((item) => namesIn(item, into));
    return into;
  }
  if (typeof value.name === 'string') into.add(value.name);
  if (typeof value.title === 'string') into.add(value.title);
  for (const child of Object.values(value)) namesIn(child, into);
  return into;
}

const outputSize = $('#output-size');
const rawCount = $('#raw-count');
const queryTime = $('#query-time');

function plainError(error) {
  return String(error?.message || error).replace(/^Error:\s*/, '').replace(/^execution error:\s*/, '');
}

function review(source = queryText()) {
  try {
    const parsed = JSON.parse(db.check(schemaText(), source));
    if (parsed && Array.isArray(parsed.diagnostics)) {
      return { diagnostics: parsed.diagnostics, text: parsed.text || '', failed: false };
    }
  } catch (e) {
    return { diagnostics: [], text: plainError(e), failed: true };
  }
  return { diagnostics: [], text: '', failed: true };
}

function mark(diags) {
  for (const [pane, editor] of [['schema', schemaEditor], ['query', queryEditor]]) {
    const markers = diags.filter((d) => d.pane === pane).map((d) => ({
      startLineNumber: d.line || 1,
      startColumn: d.column || 1,
      endLineNumber: d.endLine || d.line || 1,
      endColumn: d.endColumn || ((d.column || 1) + (d.underlineLength || 1)),
      message: d.help ? `${d.message}\n\n${d.help}` : d.message,
      severity: monaco.MarkerSeverity.Error,
    }));
    monaco.editor.setModelMarkers(editor.getModel(), 'zega', markers);
  }
}

function showJson(value, raw) {
  const text = format_json(raw);
  monaco.editor.setModelLanguage(outputEditor.getModel(), 'json');
  outputEditor.setValue(text);
  const kb = new TextEncoder().encode(text).length / 1024;
  outputSize.textContent = kb < 10 ? `${kb.toFixed(2)} KB` : `${kb.toFixed(1)} KB`;
  lastValue = value;
  drawGraph();
}

function showReport(report) {
  monaco.editor.setModelLanguage(outputEditor.getModel(), 'zega-output');
  outputEditor.setValue(report.text || '');
  outputSize.textContent = '';
}

function showThrown(error) {
  showReport({ text: plainError(error) });
}

function looksLikeZqlFile(text) {
  return /^(?:\s|\/\/[^\n]*(?:\n|$))*schema\b/.test(text);
}

async function loadSources(source, document = false, provided = {}) {
  const sources = { ...provided };
  for (const location of JSON.parse(db.load_locations(source, document))) {
    if (Object.hasOwn(sources, location)) continue;
    const response = await fetch(location, { redirect: 'error' });
    if (!response.ok) throw new Error(`Cannot read ${location}: HTTP ${response.status}`);
    sources[location] = await response.text();
  }
  return sources;
}

// `options.current`, when given, says whether the text this run started from
// is still what the panes hold; a result for older text is not shown.
async function run(source, options = {}) {
  const current = options.current || (() => true);
  saveSources();
  vectorCache.clear(); // A new run may see a changed graph: its vectors are asked for afresh.
  // A schema pane holding a ZQL file (a sample, say) is applied locally on Run.
  // Never on a remote graph: it would write the pane's mutations into the
  // customer's graph (zega#116 review).
  if (options.apply && !db.remote && looksLikeZqlFile(schemaText())) {
    try {
      const sources = db.resolvesSources ? options.sources : await loadSources(schemaText(), true, options.sources);
      const raw = await db.apply_with_sources(schemaText(), sources === undefined ? undefined : JSON.stringify(sources));
      const applied = JSON.parse(raw);
      if (!current()) return null;
      if (!String(source || '').trim()) {
        showJson(applied, raw);
        return applied;
      }
    } catch (e) {
      showThrown(e);
      return null;
    }
  }
  const report = review(source);
  if (source === queryText()) mark(report.diagnostics);
  if (report.diagnostics.length || report.failed) {
    queryTime.textContent = '';
    showReport(report);
    return null;
  }
  const started = performance.now();
  try {
    const sources = db.resolvesSources ? options.sources : await loadSources(source, false, options.sources);
    const raw = await db.run_with_sources(schemaText(), source, sources === undefined ? undefined : JSON.stringify(sources));
    if (!current()) return null;
    const elapsedUs = (performance.now() - started) * 1000;
    queryTime.textContent = elapsedUs < 1000
      ? `${Math.round(elapsedUs)} µs`
      : `${(elapsedUs / 1000).toFixed(2)} ms`;
    const value = JSON.parse(raw);
    if (!db.native) { try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); } }
    if (!options.quiet) showJson(value, raw);
    return value;
  } catch (e) {
    const failedUs = (performance.now() - started) * 1000;
    queryTime.textContent = failedUs < 1000
      ? `${Math.round(failedUs)} µs`
      : `${(failedUs / 1000).toFixed(2)} ms`;
    showThrown(e);
    return null;
  }
}

$('#btn-csv').onclick = () => {
  pauseAutoplay();
  openCsv({
    run,
    previewImport: (text) => JSON.parse(db.preview_import(text)),
    clearDatabase,
    setSchema: (text) => setQuiet(schemaEditor, text),
    setQuery: (text) => setQuiet(queryEditor, text),
    currentSchema: schemaText,
    onImported: hideTour,
  });
};
$('#btn-seed').onclick = () => reseed();
$('#btn-calgary').onclick = async () => {
  try {
    const response = await fetch('./samples/calgary.zql');
    if (!response.ok) throw new Error(`Cannot load Calgary: HTTP ${response.status}`);
    const source = await response.text();
    db.schema(source); // Validate before replacing the current sample.
    const sources = await loadSources(source, true);
    hideTour();
    await clearDatabase();
    setQuiet(schemaEditor, source);
    setQuiet(queryEditor, source.slice(source.lastIndexOf('query {')).trim());
    await execute({ apply: true, sources });
    persist();
  } catch (error) { showThrown(error); }
};
// A sample with an example bar: validate, load its CSVs, run its first
// example, and show the bar.
async function loadTourSample(key, path, label) {
  try {
    const response = await fetch(path);
    if (!response.ok) throw new Error(`Cannot load ${label}: HTTP ${response.status}`);
    const source = await response.text();
    db.schema(source);
    const sources = await loadSources(source, true);
    hideTour();
    await clearDatabase();
    setSample(key);
    setQuiet(schemaEditor, source);
    setQuiet(queryEditor, SAMPLES[key].tour[0][1]);
    await execute({ apply: true, sources });
    persist();
    setTour(SAMPLES[key].tour);
    showTourBar();
    markTour();
  } catch (error) { showThrown(error); }
}
$('#btn-flights').onclick = () => loadTourSample('flights', './samples/flights.zql', 'Flights');
$('#btn-cities').onclick = () => loadTourSample('cities', './samples/cities.zql', 'Cities');
$('#btn-tickets').onclick = async () => {
  try {
    const response = await fetch('./samples/tickets.zql');
    if (!response.ok) throw new Error(`Cannot load tickets: HTTP ${response.status}`);
    const source = await response.text();
    db.schema(source);
    hideTour(); await clearDatabase();
    setQuiet(schemaEditor, source);
    setQuiet(queryEditor, source.slice(source.lastIndexOf('query {')).trim());
    await execute({ apply: true }); persist();
  } catch (error) { showThrown(error); }
};
$('#btn-clear').onclick = async () => {
  pauseAutoplay();
  try { await clearDatabase(); } catch (error) { showThrown(error); return; }
  if (tour !== TOUR) { setTour(TOUR); hideTour(); }
  setQuiet(schemaEditor, '');
  setQuiet(queryEditor, '');
  localStorage.setItem(LS_SCHEMA, '');
  localStorage.setItem(LS_QUERY, '');
  mark([]);
  outputEditor.setValue('');
  outputSize.textContent = '';
  queryTime.textContent = '';
  drawGraph();
};

/**
 * Every run of the panes, auto or explicit, goes through here: format both
 * panes where they parse, then run what they now say. The auto-run passes
 * `format: 'auto'`, which spares whitespace being typed, or `false` after an
 * undo or redo (see sourceEdited).
 */
function execute({ format: formatFirst = true, ...options } = {}) {
  if (formatFirst) formatSources(undefined, { spareTyping: formatFirst === 'auto' });
  return run(queryText(), options);
}

// Running is the deka tour's autorun (dekaruntime/website TourLayout): one
// debounce, a version per edit so a result for older text is dropped, and a
// single rerun for edits made while a run is going instead of a pile-up.
// It pauses while the query pane holds a mutation: the graph is not reset
// between runs, so the write would repeat after every pause. Run and
// Cmd/Ctrl+Enter always run.
const AUTORUN_DEBOUNCE_MS = 350;
const autorunNote = $('#autorun-note');
let sourceVersion = 0;
let autorunTimer = null;
let autorunning = false;
let rerunRequested = false;

// Auto-run is off on a remote graph (Sami, zega#116): every call there is
// metered, so queries run only on Run or Cmd/Ctrl+Enter.
function autorunPaused() {
  const remote = Boolean(db.remote);
  const paused = remote || hasMutation(queryText());
  autorunNote.textContent = remote ? 'auto-run off: remote graph' : 'auto-run paused: mutation';
  autorunNote.hidden = !paused;
  return paused;
}

async function autorunOnce() {
  const report = review();
  mark(report.diagnostics);
  // Checked in the browser (wasm), so a remote graph still gets diagnostics; nothing is sent.
  if (db.remote) { autorunPaused(); return; }
  if (report.diagnostics.length || report.failed) {
    drawGraph();
    queryTime.textContent = '';
    showReport(report);
    return;
  }
  if (!queryText().trim() || autorunPaused()) { drawGraph(); return; }
  const version = sourceVersion;
  await execute({ format: undoneLast ? false : 'auto', current: () => version === sourceVersion });
}

async function autorun() {
  if (autorunning) { rerunRequested = true; return; }
  autorunning = true;
  try {
    do {
      rerunRequested = false;
      await autorunOnce();
    } while (rerunRequested);
  } finally {
    autorunning = false;
  }
}

function scheduleAutorun() {
  clearTimeout(autorunTimer);
  autorunTimer = setTimeout(autorun, AUTORUN_DEBOUNCE_MS);
}

// An edit to either pane reruns the query: the schema changes the result too.
// After an undo or redo the rerun leaves the text as it is: formatting it
// would push a new step onto the undo stack, so the next Cmd+Z would undo
// that instead of going further back, and redo would be lost.
let undoneLast = false;
function sourceEdited(event) {
  if (suppress || selfFormatting) return;
  undoneLast = event.isUndoing || event.isRedoing;
  sourceVersion += 1;
  pauseAutoplay();
  saveSources();
  scheduleAutorun();
}
schemaEditor.onDidChangeModelContent(sourceEdited);
queryEditor.onDidChangeModelContent(sourceEdited);
queryEditor.onDidChangeModelContent(autorunPaused);
const runNow = () => {
  clearTimeout(autorunTimer);
  return execute({ apply: true });
};
$('#btn-run').onclick = runNow;
const chord = monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter;
schemaEditor.addCommand(chord, runNow);
queryEditor.addCommand(chord, runNow);

let tourIndex = 0;
let tourTimer = null;
let playing = false;
const playBtn = $('#btn-play');
const tourBox = $('#tour-queries');

// The example bar holds the loaded sample's queries: the seed's by default.
let tour = TOUR;
function setTour(list) {
  tour = list;
  tourIndex = 0;
  tourBox.replaceChildren();
  list.forEach(([label], index) => {
    const button = document.createElement('button');
    button.textContent = label;
    button.onclick = () => {
      pauseAutoplay();
      showTour(index);
    };
    tourBox.appendChild(button);
  });
}
setTour(TOUR);

function markTour() {
  [...tourBox.children].forEach((button, index) => {
    button.classList.toggle('active', index === tourIndex);
  });
}

// A click on a tour step is an explicit run; autoplay is a timer, so it obeys
// the same mutation pause as auto-run and never writes on its own.
function showTour(index, { auto = false } = {}) {
  tourIndex = index;
  setQuiet(queryEditor, tour[index][1]);
  markTour();
  if (auto && autorunPaused()) { drawGraph(); return; }
  execute();
}

function pauseAutoplay() {
  playing = false;
  clearInterval(tourTimer);
  tourTimer = null;
  if (playBtn) playBtn.textContent = 'play';
}

function startAutoplay() {
  playing = true;
  playBtn.textContent = 'pause';
  clearInterval(tourTimer);
  tourTimer = setInterval(() => {
    showTour((tourIndex + 1) % tour.length, { auto: true });
  }, 5000);
}

playBtn.onclick = () => {
  if (playing) pauseAutoplay();
  else startAutoplay();
};

function hideTour() {
  pauseAutoplay();
  document.getElementById('tour').hidden = true;
}

function showTourBar() {
  document.getElementById('tour').hidden = false;
}

async function reseed() {
  pauseAutoplay();
  await clearDatabase();
  setQuiet(schemaEditor, SCHEMA);
  for (const seed of SEEDS) await run(seed);
  setTour(TOUR);
  showTourBar();
  showTour(0);
  startAutoplay();
}

function storedGraph() {
  return JSON.parse(db.graph());
}

function formatRaw(graph) {
  const nodes = [...graph.nodes].sort((a, b) => a.id - b.id);
  const rels = [...graph.rels].sort((a, b) => a.id - b.id);
  const nameOf = (id) => {
    const node = nodes.find((item) => item.id === id);
    if (!node) return '';
    const name = node.name ?? node.title;
    return name == null ? '' : String(name);
  };
  const sortedJson = (props) => JSON.stringify(Object.fromEntries(Object.entries(props).sort(([a], [b]) => a.localeCompare(b))));
  const propsOf = (node) => {
    const skip = new Set(['id', 'labels']);
    const props = Object.fromEntries(Object.entries(node).filter(([key]) => !skip.has(key)));
    return sortedJson(props);
  };
  const lines = [];
  for (const node of nodes) {
    lines.push(`Node ${node.id}`);
    lines.push(`  labels: [${(node.labels || []).join(', ')}]`);
    lines.push(`  props:  ${propsOf(node)}`);
    lines.push('');
  }
  for (const rel of rels) {
    const props = rel.props || {};
    lines.push(`Relationship ${rel.id}`);
    lines.push(`  kind:  ${rel.type}`);
    lines.push(`  from:  ${rel.from}  ${nameOf(rel.from)}`.trimEnd());
    lines.push(`  to:    ${rel.to}  ${nameOf(rel.to)}`.trimEnd());
    lines.push(`  props: ${sortedJson(props)}`);
    lines.push('');
  }
  return lines.join('\n').replace(/\n$/, '');
}

function showRaw(graph) {
  rawEditor.setValue(graph.nodes.length || graph.rels.length ? formatRaw(graph) : '');
  const nodes = graph.nodes.length;
  const edges = graph.rels.length;
  rawCount.textContent = nodes || edges ? `${nodes} nodes · ${edges} edges` : '';
}

function highlights(value) {
  if (value == null) return null;
  if (Array.isArray(value)) return value.length ? namesIn(value) : null;
  if (typeof value === 'object') return Object.keys(value).length ? namesIn(value) : null;
  return null;
}

function resetView() {
  disposeView?.();
  disposeView = null;
  stopSim(graphEl);
  graphEl._graph = null;
  graphEl.replaceChildren();
}

// The shared inspector: one panel for nodes and relationships alike.
function showInspector(caption, entries) {
  document.getElementById('node-inspector')?.remove();
  const panel = document.createElement('aside');
  panel.id = 'node-inspector';
  panel.setAttribute('aria-label', 'Node inspector');
  const close = document.createElement('button');
  close.textContent = 'Close';
  close.onclick = () => panel.remove();
  const title = document.createElement('h3');
  title.textContent = caption;
  const props = document.createElement('dl');
  for (const [key, value] of Object.entries(entries)) {
    const term = document.createElement('dt');
    const detail = document.createElement('dd');
    term.textContent = key;
    detail.textContent = typeof value === 'object' ? format_json(JSON.stringify(value)).trimEnd() : String(value);
    props.append(term, detail);
  }
  panel.append(close, title, props);
  graphEl.parentElement.append(panel);
}

function inspectNode(node) {
  showInspector(nodeCaption(node), Object.fromEntries(Object.entries(node).filter(([key]) => !['x', 'y', 'vx', 'vy', 'fx', 'fy', 'index'].includes(key))));
}

// A relationship in the same inspector: its kind, both ends by name, then its properties.
function inspectRel(rel) {
  const nodes = storedGraph().nodes;
  const end = (id) => { const node = nodes.find((node) => node.id === id); return node ? `${nodeCaption(node)} (${id})` : String(id); };
  showInspector(rel.type, { id: rel.id, type: rel.type, from: end(rel.from), to: end(rel.to), ...(rel.props || {}) });
}

// The relationships whose ends are both among `nodes`: what the graph draws
// as edges and the globe as arcs.
function relsAmong(rels, nodes) {
  const ids = new Set(nodes.map((node) => node.id));
  return rels.filter((rel) => ids.has(rel.from) && ids.has(rel.to));
}

// The stored nodes a result object names: by its `@id`, or by every selected
// property agreeing. An explicit `id` also distinguishes equal-valued nodes.
function matchNodes(value, nodes, types) {
  return nodes.filter((node) => {
    if (value.id != null) return value.id === node.id;
    const type = types.find((type) => node.labels.includes(type.name));
    const selected = (type?.fields || []).filter((field) => field.kind === 'prop' && field.name in value);
    return selected.length > 0 && selected.every((field) => JSON.stringify(node[field.name]) === JSON.stringify(value[field.name]));
  });
}

// Projection values carry coordinates. Resolve their stored identity for the
// shared inspector.
function mapResults(value, nodes, types, into = new Map()) {
  if (Array.isArray(value)) value.forEach((item) => mapResults(item, nodes, types, into));
  else if (value && typeof value === 'object') {
    for (const node of matchNodes(value, nodes, types)) {
      const type = types.find((type) => node.labels.includes(type.name));
      const point = type.fields.find((field) => field.kind === 'prop' && field.ty === 'Point' && value[field.name] != null);
      const coordinates = point ? value[point.name] : value;
      if (Number.isFinite(coordinates.lat) && Number.isFinite(coordinates.lon)) into.set(node.id, { ...node, lat: coordinates.lat, lon: coordinates.lon });
    }
    Object.values(value).forEach((child) => mapResults(child, nodes, types, into));
  }
  return [...into.values()];
}

// The relationships a query's result follows (zega#83): an object that names
// a stored node, then a relationship field under it whose objects name nodes
// too. `index` is the stored relationships by type and ends. The globe draws
// these in focus; null when the result follows none.
function relResults(value, nodes, types, index, into = new Set()) {
  if (Array.isArray(value)) value.forEach((item) => relResults(item, nodes, types, index, into));
  else if (value && typeof value === 'object') {
    for (const node of matchNodes(value, nodes, types)) {
      const type = types.find((type) => node.labels.includes(type.name));
      for (const field of type.fields.filter((field) => field.kind === 'edge' && value[field.field] != null)) {
        for (const item of [value[field.field]].flat()) {
          if (!item || typeof item !== 'object') continue;
          for (const other of matchNodes(item, nodes, types)) {
            const [from, to] = field.direction === 'out' ? [node.id, other.id] : [other.id, node.id];
            const id = index.get(`${field.rel}\n${from}\n${to}`);
            if (id != null) into.add(id);
          }
        }
      }
    }
    Object.values(value).forEach((child) => relResults(child, nodes, types, index, into));
  }
  return into.size ? into : null;
}

// A remote graph's /vector-view is a metered call, and drawGraph runs on every
// redraw (theme, pane changes, connecting). There, nothing is asked before a
// query has a result, and each answer is kept per (result, view, selection,
// k, threshold) so a redraw reuses it. Locally it is wasm and is not cached.
const EMPTY_VECTORS = { points: [], nearest: [], flags: [] };
const VECTOR_CACHE_SIZE = 20;
const vectorCache = new Map();
async function vectorView(kind, selected, k, threshold) {
  const result = JSON.stringify(lastValue);
  if (!db.remote) {
    const value = db.vector_view(schemaText(), result, kind, selected, k, threshold);
    return typeof value === 'string' ? JSON.parse(value) : await value;
  }
  if (lastValue == null) return EMPTY_VECTORS;
  const key = JSON.stringify([result, kind, selected ?? null, k, threshold, schemaText()]);
  if (!vectorCache.has(key)) {
    const pending = db.vector_view(schemaText(), result, kind, selected, k, threshold);
    pending.catch(() => { if (vectorCache.get(key) === pending) vectorCache.delete(key); });
    vectorCache.set(key, pending);
    if (vectorCache.size > VECTOR_CACHE_SIZE) vectorCache.delete(vectorCache.keys().next().value);
  }
  return vectorCache.get(key);
}

function drawGraph() {
  const raw = db.graph();
  const graph = JSON.parse(raw);
  const credit = sample()?.credit || '';
  showRaw(graph);
  let schema;
  try { schema = JSON.parse(db.schema(schemaText())); }
  catch {
    resetView();
    $('#view-tabs').replaceChildren();
    graphEl.textContent = schemaText().trim() ? 'Fix the schema error to display your data.' : 'No schema yet.';
    return;
  }
  const key = JSON.stringify(schema.display);
  if (key !== displayKey) {
    resetView();
    displayKey = key;
    activeView = schema.display.default;
  }
  const tabs = $('#view-tabs');
  tabs.replaceChildren();
  for (const view of schema.display.views) {
    const button = document.createElement('button');
    button.type = 'button';
    button.role = 'tab';
    button.textContent = view.kind[0].toUpperCase() + view.kind.slice(1);
    button.dataset.view = view.kind;
    button.setAttribute('aria-selected', String(view.kind === activeView));
    button.onclick = () => { activeView = view.kind; resetView(); drawGraph(); };
    tabs.append(button);
  }
  const view = schema.display.views.find((view) => view.kind === activeView);
  const types = schema.types.filter((type) => !view.types || view.types.includes(type.name));
  const allowed = new Set(types.map((type) => type.name));
  const nodes = graph.nodes.filter((node) => node.labels.some((label) => allowed.has(label)));
  if (activeView === 'graph') {
    renderGraph(graphEl, { nodes, rels: relsAmong(graph.rels, nodes) }, highlights(lastValue), {
      onNode: openNodeMenu, onEdge: openEdgeMenu, onInspect: inspectNode,
    }, view, types);
  } else if (activeView === 'table') {
    renderTable(graphEl, graph, types, inspectNode);
  } else if (activeView === 'map') {
    disposeView?.();
    disposeView = renderMap(graphEl, mapResults(lastValue, nodes, types), theme, inspectNode, credit);
  } else if (activeView === 'globe') {
    // The globe draws the stored graph, with the query's relationships in
    // focus. While that graph, the camera, the theme and the credit are
    // unchanged, a re-run refocuses the arcs and leaves the globe as it is:
    // mid-animation, and where the reader put it.
    const rels = relsAmong(graph.rels, nodes);
    const focus = relResults(lastValue, nodes, types, new Map(rels.map((rel) => [`${rel.type}\n${rel.from}\n${rel.to}`, rel.id])));
    const key = [theme, JSON.stringify(view.globe), credit, raw].join('\n');
    if (graphEl._map && key === globeKey) { graphEl._globe?.focus(focus); return; }
    globeKey = key;
    disposeView?.();
    disposeView = renderGlobe(graphEl, { ...globeData(nodes, types, rels), credit, focus }, view.globe, theme, inspectNode, inspectRel);
  } else if (activeView === 'vector2d' || activeView === 'vector3d') {
    disposeView?.();
    const analyze = (selected, k, threshold) => vectorView(activeView, selected ?? undefined, k, threshold);
    disposeView = renderVector(graphEl, { nodes, rels: graph.rels }, activeView, theme, analyze, inspectNode);
  } else if (activeView === 'timeline') {
    const list = document.createElement('ol');
    list.className = 'timeline-view';
    const dated = nodes.map((node) => ({ node, field: types.find((type) => node.labels.includes(type.name))?.timeline_field }))
      .filter(({ node, field }) => field && node[field] != null)
      .sort((a, b) => String(a.node[a.field]).localeCompare(String(b.node[b.field]), undefined, { numeric: true }));
    for (const { node, field } of dated) {
      const entry = document.createElement('li');
      const button = document.createElement('button');
      button.textContent = `${node[field]} · ${nodeCaption(node)}`;
      button.onclick = () => inspectNode(node);
      entry.append(button);
      list.append(entry);
    }
    graphEl.replaceChildren(list);
  }
}

function closeMenu() {
  document.querySelectorAll('.graph-menu').forEach((menu) => menu.remove());
}

function showMenu(x, y, rows) {
  closeMenu();
  const menu = document.createElement('div');
  menu.className = 'graph-menu';
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
  if (!rows.length) {
    const empty = document.createElement('button');
    empty.type = 'button';
    empty.disabled = true;
    empty.textContent = 'Nothing here';
    menu.appendChild(empty);
  }
  for (const row of rows) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = row.label;
    if (row.danger) button.classList.add('danger');
    button.disabled = Boolean(row.disabled);
    button.onclick = async (event) => {
      event.stopPropagation();
      if (row.menu) showMenu(x, y, row.menu);
      else {
        closeMenu();
        try { await row.run?.(); } catch (error) { showThrown(error); }
      }
    };
    menu.appendChild(button);
  }
  document.body.appendChild(menu);
}

function schemaEdges(label) {
  const type = parseSchema(schemaText()).find((item) => item.name === label);
  return (type?.fields || []).filter((field) => field.edge);
}

function nodesOf(typeName) {
  try {
    return storedGraph().nodes.filter((node) => (node.labels || []).includes(typeName));
  } catch {
    return [];
  }
}

function refreshGraph() {
  if (!db.native) { try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); } }
  const source = queryText().trim();
  if (source && !hasMutation(source)) execute();
  else {
    lastValue = null;
    drawGraph();
  }
}

function openNodeMenu(node, x, y) {
  const edges = (node.labels || []).flatMap(schemaEdges);
  showMenu(x, y, [
    { label: 'Delete Node', danger: true, run: async () => { await db.delete_node(node.id); refreshGraph(); } },
    {
      label: 'Add Edge',
      disabled: !edges.length,
      menu: edges.map((edge) => {
        const targets = nodesOf(edge.target).filter((target) => target.id !== node.id);
        return {
          label: `${edge.name} ${edge.dir} ${edge.target}`,
          menu: targets.length ? targets.map((target) => ({
            label: nodeCaption(target),
            run: async () => {
              try {
                await db.connect(schemaText(), node.id, edge.name, target.id);
                refreshGraph();
              } catch (error) {
                showThrown(error);
              }
            },
          })) : [{ label: `No ${edge.target} nodes`, disabled: true }],
        };
      }),
    },
  ]);
}

function openEdgeMenu(rel, x, y) {
  showMenu(x, y, [
    { label: 'Delete Edge', danger: true, run: async () => { await db.delete_relationship(rel.id); refreshGraph(); } },
  ]);
}

function nodeCaption(node) {
  return String(node.name ?? node.title ?? node.id);
}

const panesEl = document.getElementById('panes');
const bandTop = document.getElementById('band-top');
const bandBottom = document.getElementById('band-bottom');

function storedRatio(key, fallback) {
  const value = Number(localStorage.getItem(key));
  return Number.isFinite(value) && value > 0.15 && value < 0.85 ? value : fallback;
}

let splitTop = storedRatio('zega.v2.split-top', 0.3);
let splitRows = storedRatio('zega.v2.split-rows', 1.15 / 2);

function applySplits() {
  bandTop.style.setProperty('--lead', `${splitTop * 100}%`);
  bandTop.style.flex = String(splitRows);
  bandBottom.style.flex = String(1 - splitRows);
}
applySplits();

function dragSplit(handle, onMove) {
  handle.addEventListener('pointerdown', (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add('dragging');
    const move = (ev) => onMove(ev);
    const stop = () => {
      handle.classList.remove('dragging');
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', stop);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', stop);
  });
}

document.addEventListener('pointerdown', (event) => {
  if (!event.target.closest('.graph-menu')) closeMenu();
});
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') closeMenu();
});

document.querySelectorAll('.split-x').forEach((handle) => {
  dragSplit(handle, (ev) => {
    const band = handle.parentElement;
    const rect = band.getBoundingClientRect();
    const ratio = Math.min(0.8, Math.max(0.15, (ev.clientX - rect.left) / rect.width));
    if (handle.dataset.band === 'top') {
      splitTop = ratio;
      localStorage.setItem('zega.v2.split-top', String(ratio));
    }
    applySplits();
  });
});

dragSplit(document.getElementById('split-rows'), (ev) => {
  const rect = panesEl.getBoundingClientRect();
  splitRows = Math.min(0.8, Math.max(0.2, (ev.clientY - rect.top) / rect.height));
  localStorage.setItem('zega.v2.split-rows', String(splitRows));
  applySplits();
});

// Connect to remote graph: a Zega Cloud graph by id and API key. The key is
// held by the RemoteDatabase alone (backend.js), never stored anywhere, and
// is wiped by Disconnect or a reload. The CLI's native mode serves this page
// from 127.0.0.1, which the cloud router does not allow, so it has no button.
const remoteButton = $('#btn-remote');
const remoteDialog = $('#remote-dialog');
const remoteId = $('#remote-id');
const remoteKey = $('#remote-key');
const remoteError = $('#remote-error');
const remoteSubmit = $('#remote-connect');
// These load a sample by clearing the graph first: never on a customer's graph.
const SAMPLE_BUTTONS = ['#btn-flights', '#btn-tickets', '#btn-cities', '#btn-calgary', '#btn-seed'];
remoteButton.hidden = Boolean(localDb.native);

function showRemoteError(message) {
  remoteError.textContent = message;
  remoteError.hidden = !message;
}

remoteButton.onclick = () => {
  if (db.remote) { disconnectRemote(); return; }
  showRemoteError('');
  remoteDialog.showModal();
  remoteId.focus();
};
$('#remote-cancel').onclick = () => remoteDialog.close();
// Whatever way the dialog closes, the typed key does not stay in the page.
remoteDialog.addEventListener('close', () => { remoteKey.value = ''; showRemoteError(''); });

$('#remote-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  let remote;
  try {
    remote = new RemoteDatabase(localDb, remoteId.value.trim(), remoteKey.value.trim());
  } catch (error) {
    showRemoteError(plainError(error));
    return;
  }
  remoteSubmit.disabled = true;
  remoteSubmit.textContent = 'Connecting…';
  try {
    await remote.refresh(); // Proves the id and key before anything switches.
  } catch (error) {
    remote.forget();
    showRemoteError(plainError(error));
    return;
  } finally {
    remoteSubmit.disabled = false;
    remoteSubmit.textContent = 'Connect';
  }
  remoteDialog.close();
  await useDatabase(remote);
});

function disconnectRemote() {
  if (!db.remote) return;
  db.forget();
  return useDatabase(localDb);
}

async function useDatabase(next) {
  pauseAutoplay();
  hideTour();
  db = next;
  window.__zega = db;
  vectorCache.clear();
  const remote = Boolean(db.remote);
  const conn = $('.conn');
  conn.classList.toggle('remote', remote);
  conn.title = remote ? `api.zega.dev/g/${db.graphId}` : '';
  $('#conn-label').textContent = remote ? `connected to ${db.graphId}` : LOCAL_LABEL;
  remoteButton.textContent = remote ? 'Disconnect' : 'Connect to remote graph';
  for (const selector of SAMPLE_BUTTONS) $(selector).hidden = remote;
  lastValue = null;
  autorunPaused();
  resetView();
  drawGraph();
  // The remote snapshot was just read to prove the key; its queries wait for Run.
  if (!remote && queryText().trim() && !hasMutation(queryText())) await execute();
}

// Read what was stored before formatting saves the panes over it.
const firstVisit = !saved && !localStorage.getItem(LS_SCHEMA);
// Both panes are formatted before anything runs, including text restored
// from the last visit.
formatSources({ history: false });
autorunPaused();
const defaultSchema = schemaText().trim() === format(SCHEMA).trim();
if (db.native) {
  // Opening an existing data directory must never reseed or clear its graph.
  hideTour();
  drawGraph();
  if (queryText().trim() && !hasMutation(queryText())) await execute();
} else if (firstVisit) {
  await reseed();
} else if (defaultSchema && !savedQuery) {
  showTour(0);
  startAutoplay();
} else {
  hideTour();
  drawGraph();
  if (queryText().trim() && !hasMutation(queryText())) await execute();
  if (sample()) {
    setTour(sample().tour);
    tourIndex = tour.findIndex(([, query]) => format(query).trim() === queryText().trim());
    markTour();
    showTourBar();
  }
}
