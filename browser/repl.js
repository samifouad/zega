import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { renderGraph } from './graph.js';
import { createEditors } from './editor.js';
import { openCsv } from './csv.js';

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

  playsFor <- Player[]
}

type Player {
  name: String
  position: String
  face: String
  salary: Int

  playsFor -> Team
  born -> Country
}

type Country {
  name: String
  flag: String

  born <- Player[]
}`;

const QUERY = `{
  Country(name = "Canada") {
    name
    born <- Player {
      name
      salary
      playsFor -> Team { name }
    }
  }
}`;

const TOUR = [
  ['Canadian players', QUERY],
  ['Russia or Canada', `{
  Country(name = "Russia" || name = "Canada") {
    name
    born <- Player {
      name
      salary
      playsFor -> Team { name }
    }
  }
}`],
  ['Centers over $10M', `{
  Player(salary > 10000000 && position = "C") {
    name
    salary
    playsFor -> Team { name }
  }
}`],
  ['Oilers or Avalanche', `{
  Team(name = "Oilers" || name = "Avalanche") {
    name
    playsFor <- Player { name &since }
  }
}`],
  ['Golden Knights', `{
  Team(name = "Golden Knights") {
    name
    playsFor <- Player { name salary born -> Country { name } }
  }
}`],
  ['Germany', `{
  Country(name = "Germany") {
    name
    born <- Player { name salary playsFor -> Team { name } }
  }
}`],
  ['Hops from Canada', `{
  Country(name = "Canada") {
    born <- Player {
      name
      &hops
      playsFor -> Team { name &hops }
    }
  }
}`],
  ['Born outside Canada', `{
  Country(name != "Canada") {
    name
    born <- Player { name playsFor -> Team { name } }
  }
}`],
];

function teamSeed(name, city, abbr, players) {
  const roster = players.map(([player, position, id, salary, since]) => {
    const joined = since == null ? '' : ` &since: ${since}`;
    return `playsFor <- Player(name: "${player}" && position: "${position}" && face: "${mug(id)}" && salary: ${salary}) { name salary${joined} }`;
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
    `born <- link Player(name: "${player}") { name }`
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

const editorsReady = createEditors({
  schema: localStorage.getItem(LS_SCHEMA) || SCHEMA,
  query: localStorage.getItem(LS_QUERY) || QUERY,
});

await init();
const db = new ZegaWasm();
const EMPTY_DB = db.export_base64();
const saved = localStorage.getItem(LS_DB);
if (saved) {
  try { db.import_base64(saved); } catch (e) { console.error(e); }
}
window.__zega = db;

const { schema: schemaEditor, query: queryEditor, output: outputEditor, monaco } = await editorsReady;

let suppress = 0;
function setQuiet(editor, value) {
  suppress += 1;
  editor.setValue(value);
  suppress -= 1;
}
function schemaText() { return schemaEditor.getValue(); }
function queryText() { return queryEditor.getValue(); }

function clearDatabase() {
  db.import_base64(EMPTY_DB);
  try { localStorage.setItem(LS_DB, EMPTY_DB); } catch (e) { console.error(e); }
  lastValue = null;
}

function persist() {
  localStorage.setItem(LS_SCHEMA, schemaText());
  localStorage.setItem(LS_QUERY, queryText());
  try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); }
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

function run(source) {
  localStorage.setItem(LS_SCHEMA, schemaText());
  localStorage.setItem(LS_QUERY, queryText());
  const report = review(source);
  if (source === queryText()) mark(report.diagnostics);
  if (report.diagnostics.length || report.failed) {
    queryTime.textContent = '';
    showReport(report);
    return null;
  }
  const started = performance.now();
  try {
    const raw = db.run(schemaText(), source);
    const elapsedUs = (performance.now() - started) * 1000;
    queryTime.textContent = elapsedUs < 1000
      ? `${Math.round(elapsedUs)} µs`
      : `${(elapsedUs / 1000).toFixed(2)} ms`;
    const value = JSON.parse(raw);
    try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); }
    showJson(value);
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

$('#btn-run').onclick = () => run(queryText());
$('#btn-csv').onclick = () => {
  pauseAutoplay();
  openCsv({
    run,
    clearDatabase,
    setSchema: (text) => setQuiet(schemaEditor, text),
    setQuery: (text) => setQuiet(queryEditor, text),
  });
};
$('#btn-seed').onclick = () => reseed();
$('#btn-clear').onclick = () => {
  pauseAutoplay();
  clearDatabase();
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
      queryTime.textContent = '';
      showReport(report);
      return;
    }
    if (!source || isMutation(source)) return;
    run(source);
  }, 350);
}
schemaEditor.onDidChangeModelContent(scheduleRun);
queryEditor.onDidChangeModelContent(scheduleRun);
const runNow = () => {
  clearTimeout(pending);
  run(queryText());
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

function reseed() {
  pauseAutoplay();
  clearDatabase();
  setQuiet(schemaEditor, SCHEMA);
  for (const seed of SEEDS) run(seed);
  showTour(0);
  startAutoplay();
}

function storedGraph() {
  return JSON.parse(db.graph());
}

function highlights(value) {
  if (value == null) return null;
  if (Array.isArray(value)) return value.length ? namesIn(value) : null;
  if (typeof value === 'object') return Object.keys(value).length ? namesIn(value) : null;
  return null;
}

function drawGraph() {
  let graph;
  try {
    graph = storedGraph();
  } catch (e) {
    graphEl.innerHTML = `<div class="empty">${e}</div>`;
    return;
  }
  renderGraph(graphEl, graph, highlights(lastValue));
}

const panesEl = document.getElementById('panes');
const bandTop = document.getElementById('band-top');
const bandBottom = document.getElementById('band-bottom');

function storedRatio(key, fallback) {
  const value = Number(localStorage.getItem(key));
  return Number.isFinite(value) && value > 0.15 && value < 0.85 ? value : fallback;
}

let splitTop = storedRatio('zega.v2.split-top', 0.3);
let splitBottom = storedRatio('zega.v2.split-bottom', 0.5);
let splitRows = storedRatio('zega.v2.split-rows', 1.15 / 2);

function applySplits() {
  bandTop.style.setProperty('--lead', `${splitTop * 100}%`);
  bandBottom.style.setProperty('--lead', `${splitBottom * 100}%`);
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

document.querySelectorAll('.split-x').forEach((handle) => {
  dragSplit(handle, (ev) => {
    const band = handle.parentElement;
    const rect = band.getBoundingClientRect();
    const ratio = Math.min(0.8, Math.max(0.15, (ev.clientX - rect.left) / rect.width));
    if (handle.dataset.band === 'top') {
      splitTop = ratio;
      localStorage.setItem('zega.v2.split-top', String(ratio));
    } else {
      splitBottom = ratio;
      localStorage.setItem('zega.v2.split-bottom', String(ratio));
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

let opening = { nodes: [] };
try { opening = storedGraph(); } catch (e) { showThrown(e); }

if (!saved) {
  reseed();
} else if (opening.nodes.length) {
  showTour(0);
  startAutoplay();
} else {
  drawGraph();
}
