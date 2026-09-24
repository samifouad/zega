import init, { ZegaWasm, format_json } from './pkg/zega_wasm.js';
import { forceSimulation, forceManyBody, forceLink, forceCenter, forceCollide, forceX, forceY } from './vendor/d3-force.js';
import { SAMPLE_STATEMENTS } from './sample.js';

const LS_DB = 'zega.browser.db';
const LS_HISTORY = 'zega.browser.history';

let db;
let history = [];
try { history = JSON.parse(localStorage.getItem(LS_HISTORY) || '[]'); } catch {}

const $ = (sel) => document.querySelector(sel);

/* ---------------------------------------------------------------- engine */

await init();
db = new ZegaWasm();
const saved = localStorage.getItem(LS_DB);
if (saved) {
  try { db.import_base64(saved); } catch (e) { console.error('failed to restore saved database:', e); }
}

function persist() {
  try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error('persist failed:', e); }
}

// exposed for debugging / console use: window.__zega.query("MATCH (n) RETURN n", "")
window.__zega = () => db;

const MUTATION_RE = /^\s*(create|merge|set|delete|detach)\b/i;
const isMutation = (q) => MUTATION_RE.test(q);

function runQuery(text) {
  const t0 = performance.now();
  try {
    const rows = JSON.parse(db.query(text, ''));
    const ms = performance.now() - t0;
    if (isMutation(text)) persist();
    refreshMeta();
    return { ok: true, rows, ms };
  } catch (e) {
    return { ok: false, error: (e && e.message) || String(e), ms: performance.now() - t0 };
  }
}

/* ------------------------------------------------------------- history */

function pushHistory(q) {
  history = [q, ...history.filter((h) => h !== q)].slice(0, 30);
  localStorage.setItem(LS_HISTORY, JSON.stringify(history));
  renderHistory();
}

function renderHistory() {
  const box = $('#history');
  box.innerHTML = '';
  if (!history.length) { box.innerHTML = '<span class="dim">nothing yet</span>'; return; }
  for (const q of history) {
    const b = document.createElement('button');
    b.className = 'side-item';
    b.textContent = q.length > 40 ? q.slice(0, 40) + '…' : q;
    b.title = q;
    b.onclick = () => { $('#editor').value = q; $('#editor').focus(); };
    box.appendChild(b);
  }
}

/* ---------------------------------------------------------------- meta */

async function sideQuery(text) {
  try { return JSON.parse(db.query(text, '')); } catch { return null; }
}

async function refreshMeta() {
  const statsBox = $('#db-stats');
  const labelsBox = $('#db-labels');
  const relsBox = $('#db-reltypes');

  const [nodes] = await Promise.all([sideQuery('MATCH (n) RETURN count(*) AS c')]);
  const nodeCount = nodes && nodes[0] ? Number(nodes[0].c ?? nodes[0]['count(*)']) : '?';
  statsBox.innerHTML = `<span class="side-item">nodes<span class="side-count">${nodeCount}</span></span>`;

  const labelRows = await sideQuery('MATCH (n) RETURN labels(n) AS labels LIMIT 500');
  const labelCounts = new Map();
  if (labelRows) {
    for (const row of labelRows) {
      const labels = Array.isArray(row.labels) ? row.labels : [];
      for (const l of labels.length ? labels : ['(unlabeled)']) {
        labelCounts.set(l, (labelCounts.get(l) || 0) + 1);
      }
    }
  }
  labelsBox.innerHTML = '';
  if (!labelCounts.size) labelsBox.innerHTML = '<span class="dim">no labels yet — try the sample graph</span>';
  for (const [label, count] of [...labelCounts.entries()].sort((a, b) => b[1] - a[1])) {
    const b = document.createElement('button');
    b.className = 'side-item';
    b.innerHTML = `<span class="chip" style="background:${labelColor(label)}22;border-color:${labelColor(label)}66">${label}</span><span class="side-count">${count}</span>`;
    b.onclick = () => runInEditor(`MATCH (n:${label}) RETURN n LIMIT 25`);
    labelsBox.appendChild(b);
  }

  const relRows = await sideQuery('MATCH ()-[r]->() RETURN r LIMIT 500');
  const relCounts = new Map();
  if (relRows) {
    for (const row of relRows) {
      const r = row.r;
      if (r && typeof r.type === 'string') relCounts.set(r.type, (relCounts.get(r.type) || 0) + 1);
    }
  }
  relsBox.innerHTML = '';
  if (!relCounts.size) relsBox.innerHTML = '<span class="dim">no relationships yet</span>';
  for (const [type, count] of [...relCounts.entries()].sort((a, b) => b[1] - a[1])) {
    const b = document.createElement('button');
    b.className = 'side-item';
    b.innerHTML = `<span class="chip rel">${type}</span><span class="side-count">${count}</span>`;
    b.onclick = () => runInEditor(`MATCH ()-[r:${type}]->() RETURN r LIMIT 25`);
    relsBox.appendChild(b);
  }
}

function runInEditor(q) {
  $('#editor').value = q;
  runFromEditor();
}

/* ------------------------------------------------------------ coloring */

const PALETTE = ['#8dd3c7', '#bebada', '#fb8072', '#80b1d3', '#fdb462', '#b3de69', '#fccde5', '#bc80bd', '#ccebc5', '#ffed6f', '#a6cee3', '#fdbf6f'];
const colorByLabel = new Map();
function labelColor(label) {
  if (!label) return '#c8c8cf';
  if (!colorByLabel.has(label)) colorByLabel.set(label, PALETTE[colorByLabel.size % PALETTE.length]);
  return colorByLabel.get(label);
}

/* ------------------------------------------------------------ run flow */

function runFromEditor() {
  const text = $('#editor').value.trim();
  if (!text) return;
  pushHistory(text);
  // a cell may hold several statements separated by semicolons
  for (const stmt of text.split(';').map((s) => s.trim()).filter(Boolean)) {
    addFrame(stmt, runQuery(stmt));
  }
}

$('#btn-run').onclick = runFromEditor;
$('#btn-refresh-meta').onclick = refreshMeta;
$('#editor').addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') { e.preventDefault(); runFromEditor(); }
});

/* ------------------------------------------------------------- favorites */

const LS_FAVORITES = 'zega.browser.favorites';
let favorites = [];
try { favorites = JSON.parse(localStorage.getItem(LS_FAVORITES) || '[]'); } catch {}

function toggleFavorite(q) {
  const i = favorites.indexOf(q);
  if (i >= 0) favorites.splice(i, 1); else favorites.unshift(q);
  localStorage.setItem(LS_FAVORITES, JSON.stringify(favorites));
  renderFavorites();
  return i < 0;
}

function renderFavorites() {
  const box = $('#favorites');
  if (!box) return;
  box.innerHTML = '';
  if (!favorites.length) { box.innerHTML = '<span class="dim">star a query to save it</span>'; return; }
  for (const q of favorites) {
    const row = document.createElement('div');
    row.style.display = 'flex';
    row.style.gap = '4px';
    row.style.alignItems = 'center';
    const b = document.createElement('button');
    b.className = 'side-item';
    b.style.flex = '1';
    b.textContent = q.length > 34 ? q.slice(0, 33) + '…' : q;
    b.title = q;
    b.onclick = () => { $('#editor').value = q; $('#editor').focus(); };
    const x = document.createElement('button');
    x.className = 'mini';
    x.textContent = '✕';
    x.title = 'remove from favorites';
    x.onclick = () => { toggleFavorite(q); };
    row.appendChild(b);
    row.appendChild(x);
    box.appendChild(row);
  }
}

/* --------------------------------------------------------------- frames */

function addFrame(query, result) {
  const frame = document.createElement('div');
  frame.className = 'frame';
  $('#frames').prepend(frame);
  renderFrame(frame, query, result);

  frame.querySelector('[data-act=rerun]').onclick = () => renderFrame(frame, query, runQuery(query));
  frame.querySelector('[data-act=collapse]').onclick = (e) => {
    frame.classList.toggle('collapsed');
    e.target.textContent = frame.classList.contains('collapsed') ? '▸' : '▾';
  };
  frame.querySelector('[data-act=dismiss]').onclick = () => frame.remove();
  frame.querySelector('[data-act=download]').onclick = () => {
    if (!result.ok) return;
    download('zega-result.json', format_json(JSON.stringify(result.rows)), 'application/json');
  };
  frame.querySelector('[data-act=downloadcsv]').onclick = () => {
    if (!result.ok) return;
    download('zega-result.csv', toCsv(result.rows), 'text/csv');
  };
  frame.querySelector('[data-act=favorite]').onclick = (e) => {
    const on = toggleFavorite(query);
    e.target.textContent = on ? '★' : '☆';
  };
  frame.querySelector('[data-act=favorite]').textContent = favorites.includes(query) ? '★' : '☆';
}

function download(name, content, type) {
  const blob = new Blob([content], { type });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}

// rows -> CSV with a union-of-keys header; nested values are JSON-encoded
function toCsv(rows) {
  if (!rows.length) return '';
  const cols = [];
  const seen = new Set();
  for (const row of rows) {
    for (const k of Object.keys(row)) if (!seen.has(k)) { seen.add(k); cols.push(k); }
  }
  const cell = (v) => {
    const s = formatCell(v);
    return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s;
  };
  return [cols.join(','), ...rows.map((row) => cols.map((c) => cell(row[c])).join(','))].join('\n');
}

function renderFrame(frame, query, result) {
  const status = result.ok
    ? `${result.rows.length} row${result.rows.length === 1 ? '' : 's'} · ${Math.max(result.ms, 0.1).toFixed(1)} ms`
    : `error · ${Math.max(result.ms, 0.1).toFixed(1)} ms`;

  frame.innerHTML = `
    <div class="frame-head">
      <button class="mini" data-act="collapse" title="collapse">▾</button>
      <span class="frame-query"></span>
      <span class="frame-status">${status}</span>
      <span class="spacer"></span>
      <span class="tabs">
        <button data-view="graph">Graph</button>
        <button data-view="table">Table</button>
        <button data-view="text">Text</button>
      </span>
      <span class="frame-actions">
        <button data-act="favorite" title="save to favorites">☆</button>
        <button data-act="rerun" title="re-run">↻</button>
        <button data-act="download" title="download JSON">⤓ json</button>
        <button data-act="downloadcsv" title="download CSV">⤓ csv</button>
        <button data-act="dismiss" title="dismiss">✕</button>
      </span>
    </div>
    <div class="frame-body"></div>`;
  frame.querySelector('.frame-query').textContent = query;

  const body = frame.querySelector('.frame-body');
  if (!result.ok) {
    frame.querySelector('.tabs').style.display = 'none';
    body.innerHTML = `<div class="frame-error"></div>`;
    body.querySelector('.frame-error').textContent = result.error;
    return;
  }

  const graph = extractGraph(result.rows);
  const views = {
    graph: () => { stopSim(body); renderGraph(body, graph); },
    table: () => { stopSim(body); renderTable(body, result.rows); },
    text: () => { stopSim(body); renderText(body, result.rows); },
  };
  const tabs = frame.querySelectorAll('.tabs button');
  const pick = (name) => {
    tabs.forEach((t) => t.classList.toggle('active', t.dataset.view === name));
    views[name]();
  };
  tabs.forEach((t) => (t.onclick = () => pick(t.dataset.view)));
  pick(graph.nodes.length ? 'graph' : 'table');
}

/* --------------------------------------------------------- graph model */

function isNodeValue(v) {
  return v && typeof v === 'object' && !Array.isArray(v) &&
    Number.isInteger(v.id) && Array.isArray(v.labels);
}
function isRelValue(v) {
  return v && typeof v === 'object' && !Array.isArray(v) &&
    Number.isInteger(v.id) && typeof v.type === 'string' &&
    Number.isInteger(v.from) && Number.isInteger(v.to);
}

function extractGraph(rows) {
  const nodes = new Map();
  const rels = new Map();
  const visit = (v) => {
    if (Array.isArray(v)) return v.forEach(visit);
    if (!v || typeof v !== 'object') return;
    if (isNodeValue(v)) { if (!nodes.has(v.id)) nodes.set(v.id, v); return; }
    if (isRelValue(v)) { if (!rels.has(v.id)) rels.set(v.id, v); return; }
    for (const k of Object.keys(v)) visit(v[k]);
  };
  rows.forEach((row) => { for (const k of Object.keys(row)) visit(row[k]); });

  // relationship endpoints that were not part of the projection
  for (const r of rels.values()) {
    if (!nodes.has(r.from)) nodes.set(r.from, { id: r.from, labels: [] });
    if (!nodes.has(r.to)) nodes.set(r.to, { id: r.to, labels: [] });
  }
  return { nodes: [...nodes.values()], rels: [...rels.values()] };
}

function nodeCaption(n) {
  if (n.__stub) return `#${n.id}`;
  return String(n.name ?? n.title ?? n.id);
}

function nodeProps(n) {
  const skip = new Set(['id', 'labels', 'type', 'from', 'to', 'props']);
  return Object.fromEntries(Object.entries(n).filter(([k]) => !skip.has(k)));
}

/* -------------------------------------------------------------- layout */
/* The layout is a live d3-force simulation (vendored in ./vendor, ISC
   license) — nodes settle continuously and re-heat when dragged, the same
   feel as the neo4j browser. */

/* Layout settings, neo4j-style: the ⚙ button on a graph opens a panel of
   sliders that live-tune the running simulation. Values persist in
   localStorage and apply to every graph. `repulsion` is a multiplier on the
   size-scaled base charge; the rest are absolute force parameters. */
const PHYS_KEY = 'zega.browser.physics';
const PHYS_DEFAULTS = { repulsion: 100, linkDist: 105, pad: 10, gravity: 0 };
function loadPhys() {
  try { return { ...PHYS_DEFAULTS, ...JSON.parse(localStorage.getItem(PHYS_KEY) || '{}') }; }
  catch { return { ...PHYS_DEFAULTS }; }
}
function savePhys(phys) {
  try { localStorage.setItem(PHYS_KEY, JSON.stringify(phys)); } catch {}
}

const NODE_R = 22;

function startSimulation(nodes, links, onTick, phys) {
  // physics scale with graph size: small graphs get the gentle default,
  // dense graphs get stronger repulsion and a longer cool-down so they
  // spread into one readable cluster instead of several tight hairballs
  const baseCharge = -Math.max(140, 45 * Math.sqrt(nodes.length));
  const sim = forceSimulation(nodes)
    .force('charge', forceManyBody().strength(baseCharge * phys.repulsion / 100))
    .force('link', forceLink(links).id((d) => d.id).distance(phys.linkDist))
    .force('center', forceCenter(0, 0))
    .force('collide', forceCollide(NODE_R + phys.pad))
    // center-pull forces, strength 0 = inert until the gravity slider moves
    .force('x', forceX(0).strength(phys.gravity / 100))
    .force('y', forceY(0).strength(phys.gravity / 100))
    .alphaDecay(nodes.length > 60 ? 0.012 : 0.0228)
    .on('tick', onTick);
  sim.baseCharge = baseCharge;
  return sim;
}

/* ------------------------------------------------------------ graph UI */

function renderGraph(container, graph) {
  if (!graph.nodes.length) {
    container.innerHTML = '<div class="empty">this result contains no nodes — try a query that returns nodes and relationships</div>';
    return;
  }
  const R = NODE_R;

  const wrap = document.createElement('div');
  wrap.className = 'graph-wrap';
  wrap.innerHTML = `<svg viewBox="0 0 800 480" preserveAspectRatio="xMidYMid meet">
      <defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M 0 0 L 10 5 L 0 10 z" fill="#9a9aa4"></path>
      </marker></defs>
      <g class="viewport"></g>
    </svg><div class="graph-tip"></div>`;
  container.innerHTML = '';
  container.appendChild(overviewChips(graph));
  container.appendChild(wrap);

  const svg = wrap.querySelector('svg');
  const vp = wrap.querySelector('.viewport');
  const tip = wrap.querySelector('.graph-tip');
  const NS = 'http://www.w3.org/2000/svg';

  const state = { scale: 1, tx: 0, ty: 0, fitted: false };
  const apply = () => vp.setAttribute('transform', `translate(${state.tx},${state.ty}) scale(${state.scale})`);

  function fit() {
    const xs = graph.nodes.map((n) => n.x), ys = graph.nodes.map((n) => n.y);
    if (!xs.length) return;
    const minX = Math.min(...xs) - 70, maxX = Math.max(...xs) + 70;
    const minY = Math.min(...ys) - 70, maxY = Math.max(...ys) + 70;
    const w = maxX - minX || 1, h = maxY - minY || 1;
    state.scale = Math.min(800 / w, 480 / h, 1.4);
    state.tx = (800 - w * state.scale) / 2 - minX * state.scale;
    state.ty = (480 - h * state.scale) / 2 - minY * state.scale;
    state.fitted = true;
    apply();
  }

  // edges: curved paths trimmed to the node rims so arrowheads land on the
  // circle edge; parallel edges between the same pair fan out into lanes so
  // dense graphs don't stack them invisibly; reciprocal pairs curve opposite
  const edges = new Map();
  const lanes = new Map();
  {
    const groups = new Map();
    for (const r of graph.rels) {
      const key = r.from < r.to ? `${r.from}>${r.to}` : `${r.to}>${r.from}`;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(r.id);
    }
    for (const ids of groups.values()) {
      ids.forEach((id, i) => lanes.set(id, i - (ids.length - 1) / 2));
    }
  }
  for (const r of graph.rels) {
    const path = document.createElementNS(NS, 'path');
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', '#9a9aa4');
    path.setAttribute('stroke-width', '1.4');
    path.setAttribute('marker-end', 'url(#arrow)');
    vp.appendChild(path);

    const label = document.createElementNS(NS, 'text');
    label.setAttribute('class', 'rlab');
    label.setAttribute('font-size', '10');
    label.setAttribute('fill', '#77777f');
    label.setAttribute('text-anchor', 'middle');
    label.textContent = r.type;
    // on dense graphs the type labels are visual noise — hover the edge instead
    if (graph.rels.length > 100) label.style.display = 'none';
    label.addEventListener('pointerenter', () => { label.setAttribute('font-weight', 'bold'); });
    label.addEventListener('pointerleave', () => { label.setAttribute('font-weight', 'normal'); });
    vp.appendChild(label);
    edges.set(r.id, { path, label });
  }

  function drawEdge(r) {
    const a = graph.nodes.find((x) => x.id === r.from);
    const b = graph.nodes.find((x) => x.id === r.to);
    if (!a || !b) return;
    const dx = b.x - a.x, dy = b.y - a.y;
    const d = Math.hypot(dx, dy) || 1;
    const ux = dx / d, uy = dy / d;
    // fan parallel edges into separate lanes, and curve reciprocal edges to
    // opposite sides
    const lane = Math.max(-5, Math.min(5, lanes.get(r.id) || 0));
    const side = r.from < r.to ? 1 : -1;
    const curve = (Math.min(20, d * 0.14) + Math.abs(lane) * 15) * (lane === 0 ? side : Math.sign(lane) || side);
    const cx = (a.x + b.x) / 2 - uy * curve;
    const cy = (a.y + b.y) / 2 + ux * curve;
    const sx = a.x + ux * (R + 2), sy = a.y + uy * (R + 2);
    const ex = b.x - ux * (R + 4), ey = b.y - uy * (R + 4);
    const { path, label } = edges.get(r.id);
    path.setAttribute('d', `M ${sx} ${sy} Q ${cx} ${cy} ${ex} ${ey}`);
    // midpoint of the quadratic bezier
    const mx = 0.25 * a.x + 0.5 * cx + 0.25 * b.x;
    const my = 0.25 * a.y + 0.5 * cy + 0.25 * b.y;
    label.setAttribute('x', mx - uy * 10);
    label.setAttribute('y', my + ux * 10 - 3);
  }

  const circles = new Map();
  for (const n of graph.nodes) {
    const g = document.createElementNS(NS, 'g');
    g.style.cursor = 'pointer';

    const circle = document.createElementNS(NS, 'circle');
    circle.setAttribute('r', R);
    circle.setAttribute('fill', labelColor(n.labels[0]));
    circle.setAttribute('stroke', 'rgba(0,0,0,0.25)');
    g.appendChild(circle);

    // caption below the node, neo4j style: clear of the rim and backed by a
    // halo so relationship lines don't strike through the text
    const text = document.createElementNS(NS, 'text');
    text.setAttribute('class', 'cap');
    text.setAttribute('font-size', '11.5');
    text.setAttribute('text-anchor', 'middle');
    text.setAttribute('dy', R + 17);
    text.setAttribute('fill', '#3a3a42');
    const cap = nodeCaption(n);
    text.textContent = cap.length > 22 ? cap.slice(0, 21) + '…' : cap;
    g.appendChild(text);

    g.addEventListener('pointerenter', (e) => {
      const props = nodeProps(n);
      const linesTxt = Object.entries(props).map(([k, v]) => `${k}: ${JSON.stringify(v)}`).join('\n');
      tip.textContent = `<${n.labels.join(':') || 'node'}> #${n.id}` + (linesTxt ? '\n' + linesTxt : '');
      tip.style.display = 'block';
    });
    g.addEventListener('pointermove', (e) => {
      const rect = wrap.getBoundingClientRect();
      tip.style.left = e.clientX - rect.left + 12 + 'px';
      tip.style.top = e.clientY - rect.top + 12 + 'px';
    });
    g.addEventListener('pointerleave', () => { tip.style.display = 'none'; });

    vp.appendChild(g);
    circles.set(n.id, { g, circle, text, n });
  }

  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const links = graph.rels.map((r) => ({ source: r.from, target: r.to }));

  function onTick() {
    if (!state.fitted) fit();
    for (const n of graph.nodes) {
      circles.get(n.id).g.setAttribute('transform', `translate(${n.x},${n.y})`);
    }
    for (const r of graph.rels) drawEdge(r);
  }

  // the live simulation: nodes settle continuously, re-heat on drag
  const phys = loadPhys();
  const sim = startSimulation(graph.nodes, links, onTick, phys);
  container._sim = sim;

  // layout settings: neo4j-style physics sliders, live-tuning this simulation
  const gear = document.createElement('button');
  gear.className = 'graph-gear';
  gear.title = 'layout settings';
  gear.textContent = '⚙';
  const panel = document.createElement('div');
  panel.className = 'graph-physics';
  panel.innerHTML = `
    <div class="ph-row"><span>Repulsion</span><input type="range" data-p="repulsion" min="25" max="300" step="5"><b></b></div>
    <div class="ph-row"><span>Link distance</span><input type="range" data-p="linkDist" min="40" max="300" step="5"><b></b></div>
    <div class="ph-row"><span>Node padding</span><input type="range" data-p="pad" min="0" max="40" step="1"><b></b></div>
    <div class="ph-row"><span>Center pull</span><input type="range" data-p="gravity" min="0" max="12" step="1"><b></b></div>
    <div class="ph-foot"><button class="mini">reset</button></div>`;
  wrap.appendChild(gear);
  wrap.appendChild(panel);
  gear.onclick = () => panel.classList.toggle('open');

  const FMT = { repulsion: (v) => v + '%', linkDist: (v) => v, pad: (v) => v, gravity: (v) => v + '%' };
  function applyPhys() {
    sim.force('charge').strength(sim.baseCharge * phys.repulsion / 100);
    sim.force('link').distance(phys.linkDist);
    sim.force('collide').radius(NODE_R + phys.pad);
    sim.force('x').strength(phys.gravity / 100);
    sim.force('y').strength(phys.gravity / 100);
    if (sim.alpha() < 0.2) sim.alpha(0.25);
    sim.restart();
    savePhys(phys);
  }
  for (const input of panel.querySelectorAll('input')) {
    const key = input.dataset.p;
    input.value = phys[key];
    input.parentElement.querySelector('b').textContent = FMT[key](phys[key]);
    input.addEventListener('input', () => {
      phys[key] = Number(input.value);
      input.parentElement.querySelector('b').textContent = FMT[key](phys[key]);
      applyPhys();
    });
  }
  panel.querySelector('.ph-foot button').onclick = () => {
    Object.assign(phys, PHYS_DEFAULTS);
    for (const input of panel.querySelectorAll('input')) {
      const k = input.dataset.p;
      input.value = phys[k];
      input.parentElement.querySelector('b').textContent = FMT[k](phys[k]);
    }
    applyPhys();
  };

  // the first fit happens on tick #1 while nodes are still in phyllotaxis —
  // re-fit once the graph has settled, unless the user already took the view
  let userInteracted = false;
  sim.on('end', () => { if (!userInteracted) fit(); });

  // drag nodes: pin with fx/fy while dragging, release + re-heat on drop
  let drag = null;
  svg.addEventListener('pointerdown', (e) => {
    const nodeEl = e.target.closest && e.target.closest('g');
    svg.setPointerCapture(e.pointerId);
    if (nodeEl && [...circles.values()].some((c) => c.g === nodeEl)) {
      userInteracted = true;
      const entry = [...circles.values()].find((c) => c.g === nodeEl);
      drag = { node: entry.n };
      sim.alphaTarget(0.25).restart();
      drag.node.fx = drag.node.x;
      drag.node.fy = drag.node.y;
    } else {
      drag = { pan: true, sx: e.clientX, sy: e.clientY, otx: state.tx, oty: state.ty };
    }
  });
  svg.addEventListener('pointermove', (e) => {
    if (!drag) return;
    if (drag.pan) {
      userInteracted = true;
      state.tx = drag.otx + (e.clientX - drag.sx);
      state.ty = drag.oty + (e.clientY - drag.sy);
      apply();
    } else {
      const pt = toSvg(e);
      drag.node.fx = pt.x;
      drag.node.fy = pt.y;
    }
  });
  svg.addEventListener('pointerup', () => {
    if (drag && drag.node) {
      drag.node.fx = null;
      drag.node.fy = null;
      sim.alphaTarget(0);
    }
    drag = null;
  });

  svg.addEventListener('wheel', (e) => {
    e.preventDefault();
    userInteracted = true;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    state.scale = Math.min(4, Math.max(0.1, state.scale * factor));
    apply();
  }, { passive: false });

  function toSvg(e) {
    const rect = svg.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * 800;
    const y = ((e.clientY - rect.top) / rect.height) * 480;
    return { x: (x - state.tx) / state.scale, y: (y - state.ty) / state.scale };
  }
}

function stopSim(container) {
  if (container._sim) {
    container._sim.stop();
    container._sim = null;
  }
}

function overviewChips(graph) {
  const el = document.createElement('div');
  el.className = 'overview';
  const labelCounts = new Map();
  const relCounts = new Map();
  for (const n of graph.nodes) {
    const key = n.labels.join(':') || '(node)';
    labelCounts.set(key, (labelCounts.get(key) || 0) + 1);
  }
  for (const r of graph.rels) relCounts.set(r.type, (relCounts.get(r.type) || 0) + 1);

  let html = '';
  if (labelCounts.size) {
    html += '<span class="overview-label">Nodes</span>';
    for (const [label, count] of [...labelCounts.entries()].sort((a, b) => b[1] - a[1])) {
      html += `<span class="chip" style="background:${labelColor(label.split(':')[0])}33;border-color:${labelColor(label.split(':')[0])}88">${label} (${count})</span>`;
    }
  }
  if (relCounts.size) {
    html += '<span class="overview-label" style="margin-left:8px">Relationships</span>';
    for (const [type, count] of relCounts) {
      html += `<span class="chip rel">${type} (${count})</span>`;
    }
  }
  el.innerHTML = html;
  return el;
}

/* -------------------------------------------------------- table + text */

function formatCell(v) {
  if (v === null || v === undefined) return 'null';
  if (typeof v === 'object') return format_json(JSON.stringify(v)).trimEnd();
  return String(v);
}

function renderTable(container, rows) {
  if (!rows.length) {
    container.innerHTML = '<div class="empty">(no rows)</div>';
    return;
  }
  const cols = [];
  const seen = new Set();
  for (const row of rows) {
    for (const k of Object.keys(row)) {
      if (!seen.has(k)) { seen.add(k); cols.push(k); }
    }
  }
  const table = document.createElement('table');
  table.className = 'result';
  table.innerHTML = `<thead><tr>${cols.map((c) => `<th>${c}</th>`).join('')}</tr></thead>` +
    `<tbody>${rows.map((row) => `<tr>${cols.map((c) => `<td>${escapeHtml(formatCell(row[c]))}</td>`).join('')}</tr>`).join('')}</tbody>`;
  container.innerHTML = '';
  container.appendChild(table);
}

function renderText(container, rows) {
  const pre = document.createElement('pre');
  pre.className = 'result-json';
  pre.textContent = format_json(JSON.stringify(rows));
  container.innerHTML = '';
  container.appendChild(pre);
}

function escapeHtml(s) {
  return s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
}

/* -------------------------------------------------------- sample + io */

$('#btn-sample').onclick = () => {
  const failures = [];
  for (const stmt of SAMPLE_STATEMENTS) {
    const r = runQuery(stmt);
    if (!r.ok) failures.push(`${stmt}\n  → ${r.error}`);
  }
  if (failures.length) {
    addFrame('-- load sample graph --', { ok: false, error: failures.join('\n\n'), ms: 0 });
  }
  runInEditor('MATCH (a:Person)-[r]->(m:Movie) RETURN a, r, m');
};

$('#btn-export').onclick = () => {
  const blob = new Blob([db.export_base64()], { type: 'text/plain' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = `zega-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.b64`;
  a.click();
  URL.revokeObjectURL(a.href);
};

$('#btn-import').onclick = () => $('#import-file').click();
$('#import-file').addEventListener('change', async (e) => {
  const file = e.target.files[0];
  if (!file) return;
  try {
    db.import_base64(await file.text());
    persist();
    refreshMeta();
    addFrame(`-- import ${file.name} --`, { ok: true, rows: [], ms: 0 });
  } catch (err) {
    addFrame(`-- import ${file.name} --`, { ok: false, error: String(err && err.message || err), ms: 0 });
  }
  e.target.value = '';
});

$('#btn-clear').onclick = () => {
  if (!confirm('Delete all nodes, relationships and keys in this browser database?')) return;
  db = new ZegaWasm();
  persist();
  refreshMeta();
  addFrame('-- clear --', { ok: true, rows: [], ms: 0 });
};

/* ---------------------------------------------------------------- boot */

renderHistory();
renderFavorites();
refreshMeta();
$('#editor').focus();
