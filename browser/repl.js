import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { renderGraph } from './graph.js';

const LS_DB = 'zega.v2.db';
const LS_SCHEMA = 'zega.v2.schema';
const LS_QUERY = 'zega.v2.query';

const SCHEMA = `type Author {
  name: String
  died?: Int

  wrote -> Book[]
}

type Book {
  title: String
  pages: Int

  wrote <- Author
}`;

const QUERY = `{
  Author(name: "Le Guin") {
    name
    died
    wrote -> Book(pages > 300) {
      title
      pages
    }
  }
}`;

const SEED = `mutation {
  Author(name: "Le Guin", died: 2018) {
    name
    wrote -> Book(title: "The Dispossessed", pages: 387) { title }
    wrote -> Book(title: "A Wizard of Earthsea", pages: 205) { title }
  }
}`;

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
$('#btn-seed').onclick = () => {
  run(SEED);
  queryEl.value = QUERY;
  persist();
};
$('#btn-clear').onclick = () => {
  localStorage.removeItem(LS_DB);
  location.reload();
};
queryEl.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
    e.preventDefault();
    run(queryEl.value);
  }
});
schemaEl.addEventListener('input', persist);
queryEl.addEventListener('input', persist);

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
  schemaEl.value = SCHEMA;
  run(SEED);
  queryEl.value = QUERY;
  run(QUERY);
} else {
  jsonEl.textContent = '';
  drawGraph();
}
