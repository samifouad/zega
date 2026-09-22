import { forceSimulation, forceManyBody, forceLink, forceCenter, forceCollide, forceX, forceY } from './vendor/d3-force.js';

const PALETTE = ['#8dd3c7', '#bebada', '#fb8072', '#80b1d3', '#fdb462', '#b3de69', '#fccde5', '#bc80bd', '#ccebc5', '#ffed6f', '#a6cee3', '#fdbf6f'];
const colorByLabel = new Map();

function labelColor(label) {
  if (!label) return '#c8c8cf';
  if (!colorByLabel.has(label)) colorByLabel.set(label, PALETTE[colorByLabel.size % PALETTE.length]);
  return colorByLabel.get(label);
}

function nodeCaption(node) {
  return String(node.name ?? node.title ?? node.id);
}

function nodeProps(node) {
  const skip = new Set(['id', 'labels', 'type', 'from', 'to', 'x', 'y', 'vx', 'vy', 'index', 'fx', 'fy', 'face', 'logo', 'flag']);
  return Object.fromEntries(Object.entries(node).filter(([key]) => !skip.has(key)));
}

const PHYS_KEY = 'zega.browser.physics';
const PHYS_DEFAULTS = { repulsion: 100, linkDist: 105, pad: 10, gravity: 0 };

function loadPhys() {
  try { return { ...PHYS_DEFAULTS, ...JSON.parse(localStorage.getItem(PHYS_KEY) || '{}') }; }
  catch { return { ...PHYS_DEFAULTS }; }
}

function savePhys(phys) {
  try { localStorage.setItem(PHYS_KEY, JSON.stringify(phys)); } catch { /* private mode */ }
}

const NODE_R = 22;

function startSimulation(nodes, links, onTick, phys) {
  const baseCharge = -Math.max(140, 45 * Math.sqrt(nodes.length));
  const sim = forceSimulation(nodes)
    .force('charge', forceManyBody().strength(baseCharge * phys.repulsion / 100))
    .force('link', forceLink(links).id((node) => node.id).distance(phys.linkDist))
    .force('center', forceCenter(0, 0))
    .force('collide', forceCollide(NODE_R + phys.pad))
    .force('x', forceX(0).strength(phys.gravity / 100))
    .force('y', forceY(0).strength(phys.gravity / 100))
    .alphaDecay(nodes.length > 60 ? 0.012 : 0.0228)
    .on('tick', onTick);
  sim.baseCharge = baseCharge;
  return sim;
}

export function stopSim(container) {
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
  for (const node of graph.nodes) {
    const key = (node.labels || []).join(':') || '(node)';
    labelCounts.set(key, (labelCounts.get(key) || 0) + 1);
  }
  for (const rel of graph.rels) relCounts.set(rel.type, (relCounts.get(rel.type) || 0) + 1);
  let html = '';
  if (labelCounts.size) {
    html += '<span class="overview-label">Nodes</span>';
    for (const [label, count] of [...labelCounts.entries()].sort((a, b) => b[1] - a[1])) {
      const color = labelColor(label.split(':')[0]);
      html += `<span class="chip" style="background:${color}33;border-color:${color}88">${label} (${count})</span>`;
    }
  }
  if (relCounts.size) {
    html += '<span class="overview-label" style="margin-left:8px">Relationships</span>';
    for (const [type, count] of relCounts) html += `<span class="chip rel">${type} (${count})</span>`;
  }
  el.innerHTML = html;
  return el;
}

// `active` is the set of name/title strings from the last JSON result.
// An empty set draws the whole graph. A non-empty set keeps those nodes
// bright and fades the rest, so the picture follows the query.
function graphSignature(graph) {
  const nodes = graph.nodes.map((node) => node.id).sort((a, b) => a - b).join(',');
  const rels = graph.rels.map((rel) => `${rel.id}:${rel.from}:${rel.to}:${rel.type}`).sort().join(',');
  return `${nodes}|${rels}`;
}

export function renderGraph(container, graph, activeArg = new Set()) {
  const signature = graphSignature(graph);
  if (container._graph && container._graph.signature === signature) {
    container._graph.setActive(activeArg);
    return;
  }
  const previous = container._graph?.capture();
  stopSim(container);
  container._graph = null;
  if (!graph.nodes.length) {
    container.innerHTML = '<div class="empty">no nodes yet</div>';
    return;
  }
  let active = activeArg;
  const R = NODE_R;
  const wrap = document.createElement('div');
  wrap.className = 'graph-wrap';
  wrap.innerHTML = `<svg viewBox="0 0 800 480" preserveAspectRatio="xMidYMid meet">
      <defs><marker id="zega-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
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
  const lit = (node) => active.size === 0 || active.has(nodeCaption(node));

  function fit() {
    const xs = graph.nodes.map((node) => node.x);
    const ys = graph.nodes.map((node) => node.y);
    if (!xs.length || xs.some((value) => !Number.isFinite(value))) return;
    const minX = Math.min(...xs) - 70, maxX = Math.max(...xs) + 70;
    const minY = Math.min(...ys) - 70, maxY = Math.max(...ys) + 70;
    const w = maxX - minX || 1, h = maxY - minY || 1;
    state.scale = Math.min(800 / w, 480 / h, 1.4);
    state.tx = (800 - w * state.scale) / 2 - minX * state.scale;
    state.ty = (480 - h * state.scale) / 2 - minY * state.scale;
    state.fitted = true;
    apply();
  }

  const lanes = new Map();
  {
    const groups = new Map();
    for (const rel of graph.rels) {
      const key = rel.from < rel.to ? `${rel.from}>${rel.to}` : `${rel.to}>${rel.from}`;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(rel.id);
    }
    for (const ids of groups.values()) {
      ids.forEach((id, i) => lanes.set(id, i - (ids.length - 1) / 2));
    }
  }

  const edges = new Map();
  for (const rel of graph.rels) {
    const path = document.createElementNS(NS, 'path');
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', '#9a9aa4');
    path.setAttribute('stroke-width', '1.4');
    path.setAttribute('marker-end', 'url(#zega-arrow)');
    vp.appendChild(path);
    const label = document.createElementNS(NS, 'text');
    label.setAttribute('class', 'rlab');
    label.setAttribute('font-size', '10');
    label.setAttribute('fill', '#77777f');
    label.setAttribute('text-anchor', 'middle');
    label.textContent = rel.type;
    if (graph.rels.length > 100) label.style.display = 'none';
    vp.appendChild(label);
    edges.set(rel.id, { path, label, rel });
  }

  function drawEdge(rel) {
    const a = graph.nodes.find((node) => node.id === rel.from);
    const b = graph.nodes.find((node) => node.id === rel.to);
    if (!a || !b || !Number.isFinite(a.x) || !Number.isFinite(b.x)) return;
    const dx = b.x - a.x, dy = b.y - a.y;
    const d = Math.hypot(dx, dy) || 1;
    const ux = dx / d, uy = dy / d;
    const lane = Math.max(-5, Math.min(5, lanes.get(rel.id) || 0));
    const side = rel.from < rel.to ? 1 : -1;
    const curve = (Math.min(20, d * 0.14) + Math.abs(lane) * 15) * (lane === 0 ? side : Math.sign(lane) || side);
    const cx = (a.x + b.x) / 2 - uy * curve;
    const cy = (a.y + b.y) / 2 + ux * curve;
    const drawn = edges.get(rel.id);
    drawn.path.setAttribute('d', `M ${a.x + ux * (R + 2)} ${a.y + uy * (R + 2)} Q ${cx} ${cy} ${b.x - ux * (R + 4)} ${b.y - uy * (R + 4)}`);
    const mx = 0.25 * a.x + 0.5 * cx + 0.25 * b.x;
    const my = 0.25 * a.y + 0.5 * cy + 0.25 * b.y;
    drawn.label.setAttribute('x', mx - uy * 10);
    drawn.label.setAttribute('y', my + ux * 10 - 3);
    const on = lit(a) && lit(b);
    drawn.path.setAttribute('opacity', on ? '1' : '0.2');
    drawn.label.setAttribute('opacity', on ? '1' : '0.2');
  }

  const circles = new Map();
  for (const node of graph.nodes) {
    const g = document.createElementNS(NS, 'g');
    g.style.cursor = 'pointer';
    const on = lit(node);
    const picture = node.face || node.logo || node.flag;
    const plate = document.createElementNS(NS, 'circle');
    plate.setAttribute('r', R);
    plate.setAttribute('fill', picture ? '#fff' : labelColor((node.labels || [])[0]));
    plate.setAttribute('stroke', on && active.size ? '#1a1a1a' : 'rgba(0,0,0,0.25)');
    plate.setAttribute('stroke-width', on && active.size ? '2.5' : '1');
    g.appendChild(plate);
    if (picture) {
      const clip = document.createElementNS(NS, 'clipPath');
      clip.setAttribute('id', `mug-${node.id}`);
      const clipCircle = document.createElementNS(NS, 'circle');
      clipCircle.setAttribute('r', R - 1);
      clip.appendChild(clipCircle);
      svg.querySelector('defs').appendChild(clip);
      const image = document.createElementNS(NS, 'image');
      image.setAttribute('href', picture);
      image.setAttribute('x', -R);
      image.setAttribute('y', -R);
      image.setAttribute('width', R * 2);
      image.setAttribute('height', R * 2);
      image.setAttribute('clip-path', `url(#mug-${node.id})`);
      image.setAttribute('preserveAspectRatio', node.face ? 'xMidYMid slice' : 'xMidYMid meet');
      g.appendChild(image);
    }
    const text = document.createElementNS(NS, 'text');
    text.setAttribute('class', 'cap');
    text.setAttribute('font-size', '11.5');
    text.setAttribute('text-anchor', 'middle');
    text.setAttribute('dy', R + 17);
    text.setAttribute('fill', '#3a3a42');
    const caption = nodeCaption(node);
    text.textContent = caption.length > 22 ? caption.slice(0, 21) + '…' : caption;
    g.appendChild(text);
    g.setAttribute('opacity', on ? '1' : '0.28');
    g.addEventListener('pointerenter', (event) => {
      const props = nodeProps(node);
      const lines = Object.entries(props).map(([key, value]) => `${key}: ${JSON.stringify(value)}`).join('\n');
      tip.textContent = `<${(node.labels || []).join(':') || 'node'}> #${node.id}` + (lines ? '\n' + lines : '');
      tip.style.display = 'block';
      const rect = wrap.getBoundingClientRect();
      tip.style.left = event.clientX - rect.left + 12 + 'px';
      tip.style.top = event.clientY - rect.top + 12 + 'px';
    });
    g.addEventListener('pointermove', (event) => {
      const rect = wrap.getBoundingClientRect();
      tip.style.left = event.clientX - rect.left + 12 + 'px';
      tip.style.top = event.clientY - rect.top + 12 + 'px';
    });
    g.addEventListener('pointerleave', () => { tip.style.display = 'none'; });
    vp.appendChild(g);
    circles.set(node.id, { g, node, plate });
  }

  function paint() {
    for (const { g, node, plate } of circles.values()) {
      const on = lit(node);
      g.setAttribute('opacity', on ? '1' : '0.28');
      plate.setAttribute('stroke', on && active.size ? '#1a1a1a' : 'rgba(0,0,0,0.25)');
      plate.setAttribute('stroke-width', on && active.size ? '2.5' : '1');
    }
    for (const rel of graph.rels) {
      const a = graph.nodes.find((node) => node.id === rel.from);
      const b = graph.nodes.find((node) => node.id === rel.to);
      const drawn = edges.get(rel.id);
      if (!a || !b || !drawn) continue;
      const on = lit(a) && lit(b);
      drawn.path.setAttribute('opacity', on ? '1' : '0.2');
      drawn.label.setAttribute('opacity', on ? '1' : '0.2');
    }
  }

  function onTick() {
    if (!state.fitted) fit();
    for (const node of graph.nodes) {
      const entry = circles.get(node.id);
      if (entry && Number.isFinite(node.x)) entry.g.setAttribute('transform', `translate(${node.x},${node.y})`);
    }
    for (const rel of graph.rels) drawEdge(rel);
  }

  if (previous) {
    for (const node of graph.nodes) {
      const saved = previous.positions.get(node.id);
      if (!saved || !Number.isFinite(saved.x)) continue;
      node.x = saved.x;
      node.y = saved.y;
      node.fx = saved.fx;
      node.fy = saved.fy;
    }
    if (previous.view) {
      state.scale = previous.view.scale;
      state.tx = previous.view.tx;
      state.ty = previous.view.ty;
      state.fitted = true;
      apply();
    }
  }

  const links = graph.rels.map((rel) => ({ source: rel.from, target: rel.to }));
  const phys = loadPhys();
  const sim = startSimulation(graph.nodes, links, onTick, phys);
  container._sim = sim;

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
  const hint = document.createElement('div');
  hint.className = 'graph-hint';
  hint.textContent = 'scroll to zoom. drag to navigate.';
  wrap.appendChild(hint);
  wrap.appendChild(gear);
  wrap.appendChild(panel);
  const zoom = document.createElement('div');
  zoom.className = 'graph-zoom';
  zoom.innerHTML = `<button type="button" data-zoom="in" title="zoom in">+</button><button type="button" data-zoom="out" title="zoom out">−</button>`;
  wrap.appendChild(zoom);
  function zoomBy(factor) {
    userInteracted = true;
    state.scale = Math.min(4, Math.max(0.1, state.scale * factor));
    apply();
  }
  zoom.querySelector('[data-zoom="in"]').onclick = () => zoomBy(1.12);
  zoom.querySelector('[data-zoom="out"]').onclick = () => zoomBy(1 / 1.12);
  gear.onclick = () => panel.classList.toggle('open');
  const format = { repulsion: (value) => value + '%', linkDist: (value) => value, pad: (value) => value, gravity: (value) => value + '%' };
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
    input.parentElement.querySelector('b').textContent = format[key](phys[key]);
    input.addEventListener('input', () => {
      phys[key] = Number(input.value);
      input.parentElement.querySelector('b').textContent = format[key](phys[key]);
      applyPhys();
    });
  }
  panel.querySelector('.ph-foot button').onclick = () => {
    Object.assign(phys, PHYS_DEFAULTS);
    for (const input of panel.querySelectorAll('input')) {
      input.value = phys[input.dataset.p];
      input.parentElement.querySelector('b').textContent = format[input.dataset.p](phys[input.dataset.p]);
    }
    applyPhys();
  };

  let userInteracted = false;
  sim.on('end', () => { if (!userInteracted) fit(); });

  let drag = null;
  svg.addEventListener('pointerdown', (event) => {
    const nodeEl = event.target.closest && event.target.closest('g');
    svg.setPointerCapture(event.pointerId);
    const entry = nodeEl && [...circles.values()].find((item) => item.g === nodeEl);
    if (entry) {
      userInteracted = true;
      drag = { node: entry.node };
      sim.alphaTarget(0.25).restart();
      drag.node.fx = drag.node.x;
      drag.node.fy = drag.node.y;
    } else {
      drag = { pan: true, sx: event.clientX, sy: event.clientY, otx: state.tx, oty: state.ty };
    }
  });
  svg.addEventListener('pointermove', (event) => {
    if (!drag) return;
    if (drag.pan) {
      userInteracted = true;
      state.tx = drag.otx + (event.clientX - drag.sx);
      state.ty = drag.oty + (event.clientY - drag.sy);
      apply();
    } else {
      const point = toSvg(event);
      drag.node.fx = point.x;
      drag.node.fy = point.y;
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
  svg.addEventListener('wheel', (event) => {
    event.preventDefault();
    userInteracted = true;
    const factor = event.deltaY < 0 ? 1.12 : 1 / 1.12;
    state.scale = Math.min(4, Math.max(0.1, state.scale * factor));
    apply();
  }, { passive: false });

  function toSvg(event) {
    const rect = svg.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width) * 800;
    const y = ((event.clientY - rect.top) / rect.height) * 480;
    return { x: (x - state.tx) / state.scale, y: (y - state.ty) / state.scale };
  }

  container._graph = {
    signature,
    setActive(next) {
      active = next;
      paint();
    },
    capture() {
      return {
        positions: new Map(graph.nodes.map((node) => [node.id, {
          x: node.x, y: node.y, fx: node.fx, fy: node.fy,
        }])),
        view: { scale: state.scale, tx: state.tx, ty: state.ty },
      };
    },
  };
}
