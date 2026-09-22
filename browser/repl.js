import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { renderGraph } from './graph.js';

const LS_DB = 'zega.v2.nhl';
const LS_SCHEMA = 'zega.v2.schema';
const LS_QUERY = 'zega.v2.query';

const mug = (id) => `https://assets.nhle.com/mugs/nhl/latest/${id}.png`;
const logo = (abbr) => `https://assets.nhle.com/logos/nhl/svg/${abbr}_light.svg`;

const SCHEMA = `type Team {
  name: String
  city: String
  logo: String

  roster -> Player[]
}

type Player {
  name: String
  position: String
  face: String

  roster <- Team
}`;

const QUERY = `{
  Team(name: "Oilers") {
    name
    roster -> Player { name position }
  }
}`;

function teamSeed(name, city, abbr, players) {
  const roster = players.map(([player, position, id]) =>
    `roster -> Player(name: "${player}", position: "${position}", face: "${mug(id)}") { name }`
  ).join('\n    ');
  return `mutation {
  Team(name: "${name}", city: "${city}", logo: "${logo(abbr)}") {
    name
    ${roster}
  }
}`;
}

const SEEDS = [
  teamSeed('Oilers', 'Edmonton', 'EDM', [
    ['Connor McDavid', 'C', 8478402],
    ['Leon Draisaitl', 'C', 8477934],
  ]),
  teamSeed('Maple Leafs', 'Toronto', 'TOR', [
    ['Auston Matthews', 'C', 8479318],
    ['Mitch Marner', 'RW', 8478483],
  ]),
  teamSeed('Avalanche', 'Colorado', 'COL', [
    ['Nathan MacKinnon', 'C', 8477492],
    ['Cale Makar', 'D', 8480069],
  ]),
  teamSeed('Penguins', 'Pittsburgh', 'PIT', [
    ['Sidney Crosby', 'C', 8471675],
  ]),
  teamSeed('Capitals', 'Washington', 'WSH', [
    ['Alex Ovechkin', 'LW', 8471214],
  ]),
  teamSeed('Lightning', 'Tampa Bay', 'TBL', [
    ['Nikita Kucherov', 'RW', 8476453],
    ['Brayden Point', 'C', 8478010],
  ]),
];

const $ = (sel) => document.querySelector(sel);
const schemaEl = $('#schema');
const queryEl = $('#query');
const jsonEl = $('#json');
const graphEl = $('#graph');

schemaEl.value = localStorage.getItem(LS_SCHEMA) || SCHEMA;
queryEl.value = localStorage.getItem(LS_QUERY) || QUERY;

await init();
const db = new ZegaWasm();
const saved = localStorage.getItem(LS_DB);
if (saved) {
  try { db.import_base64(saved); } catch (e) { console.error(e); }
}
window.__zega = db;

function persist() {
  localStorage.setItem(LS_SCHEMA, schemaEl.value);
  localStorage.setItem(LS_QUERY, queryEl.value);
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

function show(value, error) {
  jsonEl.classList.toggle('error', Boolean(error));
  jsonEl.textContent = error ? String(error) : JSON.stringify(value, null, 2);
  lastValue = error ? null : value;
  drawGraph();
}

function run(source) {
  localStorage.setItem(LS_SCHEMA, schemaEl.value);
  localStorage.setItem(LS_QUERY, queryEl.value);
  try {
    const value = JSON.parse(db.run(schemaEl.value, source));
    try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); }
    show(value, null);
    return value;
  } catch (e) {
    show(null, e);
    return null;
  }
}

$('#btn-run').onclick = () => run(queryEl.value);
$('#btn-seed').onclick = () => reseed();
$('#btn-clear').onclick = () => {
  localStorage.removeItem(LS_DB);
  localStorage.removeItem(LS_SCHEMA);
  localStorage.removeItem(LS_QUERY);
  location.reload();
};
queryEl.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
    e.preventDefault();
    clearTimeout(pending);
    run(queryEl.value);
  }
});

let pending = null;
function scheduleRun() {
  localStorage.setItem(LS_SCHEMA, schemaEl.value);
  localStorage.setItem(LS_QUERY, queryEl.value);
  clearTimeout(pending);
  pending = setTimeout(() => {
    const source = queryEl.value.trim();
    if (!source || source.startsWith('mutation')) return;
    run(source);
  }, 350);
}
schemaEl.addEventListener('input', scheduleRun);
queryEl.addEventListener('input', scheduleRun);

function reseed() {
  schemaEl.value = SCHEMA;
  for (const seed of SEEDS) run(seed);
  queryEl.value = QUERY;
  run(QUERY);
}

function storedGraph() {
  return JSON.parse(db.graph());
}

function drawGraph() {
  let graph;
  try {
    graph = storedGraph();
  } catch (e) {
    graphEl.innerHTML = `<div class="empty">${e}</div>`;
    return;
  }
  renderGraph(graphEl, graph, namesIn(lastValue));
}

let opening = { nodes: [] };
try { opening = storedGraph(); } catch (e) { jsonEl.textContent = String(e); }

if (!opening.nodes.length) {
  reseed();
} else {
  jsonEl.textContent = '';
  drawGraph();
}
