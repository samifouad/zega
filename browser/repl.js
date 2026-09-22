import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { forceSimulation, forceManyBody, forceLink, forceCenter, forceCollide } from './vendor/d3-force.js';

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

function show(value, error) {
  jsonEl.classList.toggle('error', Boolean(error));
  jsonEl.textContent = error ? String(error) : JSON.stringify(value, null, 2);
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

const PALETTE = ['#8dd3c7', '#bebada', '#fb8072', '#80b1d3', '#fdb462', '#b3de69', '#fccde5', '#bc80bd'];
const colorOf = new Map();
function labelColor(label) {
  if (!label) return '#d0d0d6';
  if (!colorOf.has(label)) colorOf.set(label, PALETTE[colorOf.size % PALETTE.length]);
  return colorOf.get(label);
}

function drawGraph() {
  if (graphEl._sim) {
    graphEl._sim.stop();
    graphEl._sim = null;
  }
  let graph;
  try { graph = JSON.parse(db.graph()); } catch { graph = { nodes: [], rels: [] }; }
  graphEl.innerHTML = '';
  if (!graph.nodes.length) {
    graphEl.innerHTML = '<div class="empty">no nodes yet</div>';
    return;
  }
  const wrap = document.createElement('div');
  wrap.className = 'graph-wrap';
  wrap.innerHTML = `<svg viewBox="0 0 800 480" preserveAspectRatio="xMidYMid meet">
    <defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="#9a9aa4"></path>
    </marker></defs><g class="viewport"></g></svg>`;
  graphEl.appendChild(wrap);
  const svg = wrap.querySelector('svg');
  const vp = wrap.querySelector('.viewport');
  const NS = 'http://www.w3.org/2000/svg';
  const R = 22;
  const state = { scale: 1, tx: 0, ty: 0, fitted: false };
  const apply = () => vp.setAttribute('transform', `translate(${state.tx},${state.ty}) scale(${state.scale})`);
  const edges = new Map();
  for (const rel of graph.rels) {
    const path = document.createElementNS(NS, 'path');
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', '#9a9aa4');
    path.setAttribute('stroke-width', '1.4');
    path.setAttribute('marker-end', 'url(#arrow)');
    const label = document.createElementNS(NS, 'text');
    label.setAttribute('class', 'rlab');
    label.setAttribute('font-size', '10');
    label.setAttribute('fill', '#77777f');
    label.setAttribute('text-anchor', 'middle');
    label.textContent = rel.type;
    vp.appendChild(path);
    vp.appendChild(label);
    edges.set(rel.id, { path, label });
  }
  const circles = new Map();
  for (const node of graph.nodes) {
    const g = document.createElementNS(NS, 'g');
    const circle = document.createElementNS(NS, 'circle');
    circle.setAttribute('r', R);
    circle.setAttribute('fill', labelColor(node.labels[0]));
    circle.setAttribute('stroke', 'rgba(0,0,0,0.25)');
    const text = document.createElementNS(NS, 'text');
    text.setAttribute('class', 'cap');
    text.setAttribute('font-size', '11.5');
    text.setAttribute('text-anchor', 'middle');
    text.setAttribute('dy', R + 17);
    text.setAttribute('fill', '#3a3a42');
    const caption = String(node.name ?? node.title ?? node.id);
    text.textContent = caption.length > 22 ? caption.slice(0, 21) + '…' : caption;
    g.appendChild(circle);
    g.appendChild(text);
    vp.appendChild(g);
    circles.set(node.id, g);
  }
  function drawEdge(rel) {
    const a = graph.nodes.find((node) => node.id === rel.from);
    const b = graph.nodes.find((node) => node.id === rel.to);
    if (!a || !b) return;
    const dx = b.x - a.x, dy = b.y - a.y;
    const d = Math.hypot(dx, dy) || 1;
    const ux = dx / d, uy = dy / d;
    const edge = edges.get(rel.id);
    edge.path.setAttribute('d', `M ${a.x + ux * (R + 2)} ${a.y + uy * (R + 2)} L ${b.x - ux * (R + 4)} ${b.y - uy * (R + 4)}`);
    edge.label.setAttribute('x', (a.x + b.x) / 2);
    edge.label.setAttribute('y', (a.y + b.y) / 2 - 6);
  }
  function fit() {
    const xs = graph.nodes.map((node) => node.x);
    const ys = graph.nodes.map((node) => node.y);
    const minX = Math.min(...xs) - 70, maxX = Math.max(...xs) + 70;
    const minY = Math.min(...ys) - 70, maxY = Math.max(...ys) + 70;
    const w = maxX - minX || 1, h = maxY - minY || 1;
    state.scale = Math.min(800 / w, 480 / h, 1.4);
    state.tx = (800 - w * state.scale) / 2 - minX * state.scale;
    state.ty = (480 - h * state.scale) / 2 - minY * state.scale;
    state.fitted = true;
    apply();
  }
  const links = graph.rels.map((rel) => ({ source: rel.from, target: rel.to }));
  const sim = forceSimulation(graph.nodes)
    .force('charge', forceManyBody().strength(-280))
    .force('link', forceLink(links).id((node) => node.id).distance(120))
    .force('center', forceCenter(0, 0))
    .force('collide', forceCollide(R + 8))
    .on('tick', () => {
      if (!state.fitted) fit();
      for (const node of graph.nodes) {
        circles.get(node.id).setAttribute('transform', `translate(${node.x},${node.y})`);
      }
      for (const rel of graph.rels) drawEdge(rel);
    });
  graphEl._sim = sim;
  sim.on('end', fit);
}

jsonEl.textContent = '';
drawGraph();
