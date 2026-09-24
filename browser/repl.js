import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { renderGraph, stopSim } from './graph.js';
import { renderMap } from './map.js';
import { renderVector } from './vector.js';
import { renderTable } from './table.js';
import { applyTheme } from './theme.js';
import { createEditors } from './editor.js';
import { openCsv, parseSchema } from './csv.js';
import { connectDatabase } from './backend.js';

const LS_DB = 'zega.v2.since';
const LS_SCHEMA = 'zega.v2.schema';
const LS_QUERY = 'zega.v2.query';

const mug = (id) => `https://assets.nhle.com/mugs/nhl/latest/${id}.png`;
const logo = (abbr) => `https://assets.nhle.com/logos/nhl/svg/${abbr}_light.svg`;
const flag = (code) => `https://flagcdn.com/w160/${code}.png`;

const SCHEMA = `type Team {
  name: String
  city: String
  logo: String

  playsFor -> Player[] {
    since?: Int
  }
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
    playsFor -> Player { name salary born <- Country { name } }
  }
}`],
  ['Germany', `{
  Country(name = "Germany") {
    name
    born -> Player { name salary playsFor <- Team { name } }
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
    born -> Player { name playsFor <- Team { name } }
  }
}`],
];

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
let activeView = null, displayKey = '', disposeView = null;


const editorsReady = createEditors({
  schema: localStorage.getItem(LS_SCHEMA) || SCHEMA,
  query: localStorage.getItem(LS_QUERY) || QUERY,
});

await init();
const db = await connectDatabase(new ZegaWasm());
if (db.native) document.querySelector('.conn').innerHTML = '<span class="conn-dot"></span> local · native';
const EMPTY_DB = db.native ? null : db.export_base64();
const saved = db.native ? null : localStorage.getItem(LS_DB);
if (saved) {
  try { db.import_base64(saved); } catch (e) { console.error(e); }
}
window.__zega = db;

const { schema: schemaEditor, query: queryEditor, output: outputEditor, raw: rawEditor, monaco } = await editorsReady;

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

let suppress = 0;
function setQuiet(editor, value) {
  suppress += 1;
  editor.setValue(value);
  suppress -= 1;
}
function schemaText() { return schemaEditor.getValue(); }
function queryText() { return queryEditor.getValue(); }

async function clearDatabase() {
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

function showJson(value) {
  const text = JSON.stringify(value, null, 2);
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

async function run(source, options = {}) {
  localStorage.setItem(LS_SCHEMA, schemaText());
  localStorage.setItem(LS_QUERY, queryText());
  if (options.apply && looksLikeZqlFile(schemaText())) {
    try {
      const sources = db.native ? options.sources : await loadSources(schemaText(), true, options.sources);
      const applied = JSON.parse(await db.apply_with_sources(schemaText(), sources === undefined ? undefined : JSON.stringify(sources)));
      if (!String(source || '').trim()) {
        showJson(applied);
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
    const sources = db.native ? options.sources : await loadSources(source, false, options.sources);
    const raw = await db.run_with_sources(schemaText(), source, sources === undefined ? undefined : JSON.stringify(sources));
    const elapsedUs = (performance.now() - started) * 1000;
    queryTime.textContent = elapsedUs < 1000
      ? `${Math.round(elapsedUs)} µs`
      : `${(elapsedUs / 1000).toFixed(2)} ms`;
    const value = JSON.parse(raw);
    if (!db.native) { try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); } }
    if (!options.quiet) showJson(value);
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

$('#btn-run').onclick = () => run(queryText(), { apply: true });
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
    await run(queryText(), { apply: true, sources });
    persist();
  } catch (error) { showThrown(error); }
};
$('#btn-tickets').onclick = async () => {
  try {
    const response = await fetch('./samples/tickets.zql');
    if (!response.ok) throw new Error(`Cannot load tickets: HTTP ${response.status}`);
    const source = await response.text();
    db.schema(source);
    hideTour(); await clearDatabase();
    setQuiet(schemaEditor, source);
    setQuiet(queryEditor, source.slice(source.lastIndexOf('query {')).trim());
    await run(queryText(), { apply: true }); persist();
  } catch (error) { showThrown(error); }
};
$('#btn-clear').onclick = async () => {
  pauseAutoplay();
  await clearDatabase();
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

let pending = null;
function isMutation(source) {
  return /^mutation\b/.test(source.replace(/\/\/.*$/gm, '').trim());
}
function scheduleRun() {
  if (suppress) return;
  pauseAutoplay();
  localStorage.setItem(LS_SCHEMA, schemaText());
  localStorage.setItem(LS_QUERY, queryText());
  clearTimeout(pending);
  pending = setTimeout(() => {
    const source = queryText().trim();
    const report = review();
    mark(report.diagnostics);
    if (report.diagnostics.length || report.failed) {
      drawGraph();
      queryTime.textContent = '';
      showReport(report);
      return;
    }
    if (!source || isMutation(source)) { drawGraph(); return; }
    run(source);
  }, 350);
}
schemaEditor.onDidChangeModelContent(scheduleRun);
queryEditor.onDidChangeModelContent(scheduleRun);
const runNow = () => {
  clearTimeout(pending);
  run(queryText(), { apply: true });
};
const chord = monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter;
schemaEditor.addCommand(chord, runNow);
queryEditor.addCommand(chord, runNow);

let tourIndex = 0;
let tourTimer = null;
let playing = false;
const playBtn = $('#btn-play');
const tourBox = $('#tour-queries');

TOUR.forEach(([label], index) => {
  const button = document.createElement('button');
  button.textContent = label;
  button.onclick = () => {
    pauseAutoplay();
    showTour(index);
  };
  tourBox.appendChild(button);
});

function markTour() {
  [...tourBox.children].forEach((button, index) => {
    button.classList.toggle('active', index === tourIndex);
  });
}

function showTour(index) {
  tourIndex = index;
  setQuiet(queryEditor, TOUR[index][1]);
  markTour();
  run(TOUR[index][1]);
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
    showTour((tourIndex + 1) % TOUR.length);
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

function inspectNode(node) {
  document.getElementById('node-inspector')?.remove();
  const panel = document.createElement('aside');
  panel.id = 'node-inspector';
  panel.setAttribute('aria-label', 'Node inspector');
  const close = document.createElement('button');
  close.textContent = 'Close';
  close.onclick = () => panel.remove();
  const title = document.createElement('h3');
  title.textContent = nodeCaption(node);
  const props = document.createElement('dl');
  for (const [key, value] of Object.entries(node)) {
    if (['x', 'y', 'vx', 'vy', 'fx', 'fy', 'index'].includes(key)) continue;
    const term = document.createElement('dt');
    const detail = document.createElement('dd');
    term.textContent = key;
    detail.textContent = typeof value === 'object' ? JSON.stringify(value) : String(value);
    props.append(term, detail);
  }
  panel.append(close, title, props);
  graphEl.parentElement.append(panel);
}

// Projection values carry coordinates. Resolve their stored identity for the
// shared inspector; an explicit `id` also distinguishes equal-valued nodes.
function mapResults(value, nodes, types, into = new Map()) {
  if (Array.isArray(value)) value.forEach((item) => mapResults(item, nodes, types, into));
  else if (value && typeof value === 'object') {
    for (const node of nodes) {
      const type = types.find((type) => node.labels.includes(type.name));
      const fields = type?.fields.filter((field) => field.kind === 'prop') || [];
      const point = fields.find((field) => field.ty === 'Point' && value[field.name] != null);
      const coordinates = point ? value[point.name] : value;
      if (!Number.isFinite(coordinates.lat) || !Number.isFinite(coordinates.lon)) continue;
      const matches = value.id != null ? value.id === node.id : fields
        .filter((field) => field.name in value)
        .every((field) => JSON.stringify(node[field.name]) === JSON.stringify(value[field.name]));
      if (matches && (value.id != null || fields.some((field) => field.name in value))) {
        into.set(node.id, { ...node, lat: coordinates.lat, lon: coordinates.lon });
      }
    }
    Object.values(value).forEach((child) => mapResults(child, nodes, types, into));
  }
  return [...into.values()];
}

function drawGraph() {
  const graph = storedGraph();
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
    const ids = new Set(nodes.map((node) => node.id));
    renderGraph(graphEl, { nodes, rels: graph.rels.filter((rel) => ids.has(rel.from) && ids.has(rel.to)) }, highlights(lastValue), {
      onNode: openNodeMenu, onEdge: openEdgeMenu, onInspect: inspectNode,
    });
  } else if (activeView === 'table') {
    renderTable(graphEl, graph, types, inspectNode);
  } else if (activeView === 'map') {
    disposeView?.();
    disposeView = renderMap(graphEl, mapResults(lastValue, nodes, types), theme, inspectNode);
  } else if (activeView === 'vector2d' || activeView === 'vector3d') {
    disposeView?.();
    const analyze = async (selected, k, threshold) => {
      const value = db.vector_view(schemaText(), JSON.stringify(lastValue), activeView, selected ?? undefined, k, threshold);
      return typeof value === 'string' ? JSON.parse(value) : await value;
    };
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
  if (source && !isMutation(source)) run(source);
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

const defaultSchema = schemaText().trim() === SCHEMA.trim();
if (db.native) {
  // Opening an existing data directory must never reseed or clear its graph.
  hideTour();
  drawGraph();
  if (queryText().trim() && !isMutation(queryText())) await run(queryText());
} else if (!saved && !localStorage.getItem(LS_SCHEMA)) {
  await reseed();
} else if (defaultSchema) {
  showTour(0);
  startAutoplay();
} else {
  hideTour();
  drawGraph();
  if (queryText().trim() && !isMutation(queryText())) await run(queryText());
}
