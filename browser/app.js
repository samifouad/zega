import init, { ZegaWasm } from './pkg/zega_wasm.js';
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

const MUTATION_RE = /^\s*(create|merge|set|delete|detach|del|incr)\b/i;
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
    download('zega-result.json', JSON.stringify(result.rows, null, 2), 'application/json');
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
    graph: () => renderGraph(body, graph),
    table: () => renderTable(body, result.rows),
    text: () => renderText(body, result.rows),
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

function layoutGraph(nodes, rels) {
  const idx = new Map(nodes.map((n) => [n.id, n]));
  nodes.forEach((n, i) => {
    const a = (i / Math.max(nodes.length, 1)) * 2 * Math.PI;
    n.x = Math.cos(a) * 200;
    n.y = Math.sin(a) * 200;
    n.vx = 0; n.vy = 0;
  });
  // node radius + padding: two circles closer than this get pushed apart,
  // the d3 "collide" force that keeps nodes from overlapping
  const COLLIDE = 62;
  for (let iter = 0; iter < 300; iter++) {
    for (let i = 0; i < nodes.length; i++) {
      for (let j = i + 1; j < nodes.length; j++) {
        const dx = nodes[i].x - nodes[j].x;
        const dy = nodes[i].y - nodes[j].y;
        const d2 = dx * dx + dy * dy || 1;
        const d = Math.sqrt(d2);
        const f = 2600 / d2;
        const fx = (f * dx) / d, fy = (f * dy) / d;
        nodes[i].vx += fx; nodes[i].vy += fy;
        nodes[j].vx -= fx; nodes[j].vy -= fy;
      }
    }
    for (const r of rels) {
      const a = idx.get(r.from), b = idx.get(r.to);
      if (!a || !b) continue;
      const dx = b.x - a.x, dy = b.y - a.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const f = (d - 110) * 0.02;
      const fx = (f * dx) / d, fy = (f * dy) / d;
      a.vx += fx; a.vy += fy;
      b.vx -= fx; b.vy -= fy;
    }
    for (const n of nodes) {
      n.vx = (n.vx - n.x * 0.008) * 0.85;
      n.vy = (n.vy - n.y * 0.008) * 0.85;
      n.x += n.vx; n.y += n.vy;
    }
    // collision: positional correction, two passes for stability
    for (let pass = 0; pass < 2; pass++) {
      for (let i = 0; i < nodes.length; i++) {
        for (let j = i + 1; j < nodes.length; j++) {
          const dx = nodes[j].x - nodes[i].x;
          const dy = nodes[j].y - nodes[i].y;
          const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
          if (d < COLLIDE) {
            const push = (COLLIDE - d) / 2;
            const ux = dx / d, uy = dy / d;
            nodes[i].x -= ux * push; nodes[i].y -= uy * push;
            nodes[j].x += ux * push; nodes[j].y += uy * push;
          }
        }
      }
    }
  }
}

/* ------------------------------------------------------------ graph UI */

function renderGraph(container, graph) {
  if (!graph.nodes.length) {
    container.innerHTML = '<div class="empty">this result contains no nodes — try a query that returns nodes and relationships</div>';
    return;
  }
  layoutGraph(graph.nodes, graph.rels);

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
  const R = 22;

  const state = { scale: 1, tx: 0, ty: 0 };
  const apply = () => vp.setAttribute('transform', `translate(${state.tx},${state.ty}) scale(${state.scale})`);

  // fit content into the viewBox
  {
    const xs = graph.nodes.map((n) => n.x), ys = graph.nodes.map((n) => n.y);
    const minX = Math.min(...xs) - 60, maxX = Math.max(...xs) + 60;
    const minY = Math.min(...ys) - 60, maxY = Math.max(...ys) + 60;
    const w = maxX - minX || 1, h = maxY - minY || 1;
    state.scale = Math.min(800 / w, 480 / h, 1.4);
    state.tx = (800 - w * state.scale) / 2 - minX * state.scale;
    state.ty = (480 - h * state.scale) / 2 - minY * state.scale;
  }
  apply();

  // edges: curved paths trimmed to the node rims so arrowheads land on the
  // circle edge; reciprocal edges curve to opposite sides
  const edges = new Map();
  for (const r of graph.rels) {
    const path = document.createElementNS(NS, 'path');
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', '#9a9aa4');
    path.setAttribute('stroke-width', '1.4');
    path.setAttribute('marker-end', 'url(#arrow)');
    vp.appendChild(path);

    const label = document.createElementNS(NS, 'text');
    label.setAttribute('font-size', '10');
    label.setAttribute('fill', '#77777f');
    label.setAttribute('text-anchor', 'middle');
    label.textContent = r.type;
    vp.appendChild(label);
    edges.set(r.id, { path, label });
  }

  function edgeGeometry(r) {
    const a = graph.nodes.find((x) => x.id === r.from);
    const b = graph.nodes.find((x) => x.id === r.to);
    if (!a || !b) return null;
    const dx = b.x - a.x, dy = b.y - a.y;
    const d = Math.hypot(dx, dy) || 1;
    const ux = dx / d, uy = dy / d;
    // curve side is fixed per direction so reciprocal edges separate
    const side = r.from < r.to ? 1 : -1;
    const curve = Math.min(28, d * 0.18) * side;
    const cx = (a.x + b.x) / 2 - uy * curve;
    const cy = (a.y + b.y) / 2 + ux * curve;
    return { a, b, ux, uy, cx, cy };
  }

  function drawEdge(r) {
    const e = edgeGeometry(r);
    if (!e) return;
    const { a, b, ux, uy, cx, cy } = e;
    const sx = a.x + ux * (R + 2), sy = a.y + uy * (R + 2);
    const ex = b.x - ux * (R + 4), ey = b.y - uy * (R + 4);
    const { path, label } = edges.get(r.id);
    path.setAttribute('d', `M ${sx} ${sy} Q ${cx} ${cy} ${ex} ${ey}`);
    // midpoint of the quadratic bezier
    const mx = 0.25 * a.x + 0.5 * cx + 0.25 * b.x;
    const my = 0.25 * a.y + 0.5 * cy + 0.25 * b.y;
    label.setAttribute('x', mx - uy * 8);
    label.setAttribute('y', my + ux * 8 - 3);
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

    // caption below the node, neo4j style
    const text = document.createElementNS(NS, 'text');
    text.setAttribute('font-size', '11');
    text.setAttribute('text-anchor', 'middle');
    text.setAttribute('dy', R + 14);
    text.setAttribute('fill', '#44444c');
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
    position(n);
  }

  function position(n) {
    const c = circles.get(n.id);
    c.g.setAttribute('transform', `translate(${n.x},${n.y})`);
    for (const r of graph.rels) {
      if (r.from === n.id || r.to === n.id) drawEdge(r);
    }
  }

  // drag nodes
  let drag = null;
  svg.addEventListener('pointerdown', (e) => {
    const target = e.target;
    const nodeEl = target.closest && target.closest('g');
    svg.setPointerCapture(e.pointerId);
    const pt = toSvg(e);
    if (nodeEl && [...circles.values()].some((c) => c.g === nodeEl)) {
      const entry = [...circles.values()].find((c) => c.g === nodeEl);
      drag = { node: entry.n };
    } else {
      drag = { pan: true, sx: e.clientX, sy: e.clientY, otx: state.tx, oty: state.ty };
    }
  });
  svg.addEventListener('pointermove', (e) => {
    if (!drag) return;
    if (drag.pan) {
      state.tx = drag.otx + (e.clientX - drag.sx);
      state.ty = drag.oty + (e.clientY - drag.sy);
      apply();
    } else {
      const pt = toSvg(e);
      drag.node.x = pt.x; drag.node.y = pt.y;
      position(drag.node);
    }
  });
  svg.addEventListener('pointerup', () => { drag = null; });

  svg.addEventListener('wheel', (e) => {
    e.preventDefault();
    const pt = toSvg(e);
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    state.scale = Math.min(4, Math.max(0.1, state.scale * factor));
    state.tx = pt.x * (800 / svg.clientWidth) * 0 + state.tx; // keep translate; zoom around view centre
    apply();
  }, { passive: false });

  function toSvg(e) {
    const rect = svg.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * 800;
    const y = ((e.clientY - rect.top) / rect.height) * 480;
    return { x: (x - state.tx) / state.scale, y: (y - state.ty) / state.scale };
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
  if (typeof v === 'object') return JSON.stringify(v);
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
  pre.textContent = JSON.stringify(rows, null, 2);
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
