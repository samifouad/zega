import { drawNode, nodeContent, nodeGeometry, previewNode } from './node-display.js';
import { forceSimulation, forceManyBody, forceLink, forceCenter, forceCollide, forceX, forceY } from './vendor/d3-force.js';

const PALETTE = ['#8dd3c7', '#bebada', '#fb8072', '#80b1d3', '#fdb462', '#b3de69', '#fccde5', '#bc80bd', '#ccebc5', '#ffed6f', '#a6cee3', '#fdbf6f'];
const colorByLabel = new Map();

function labelColor(label) {
  if (!label) return '#c8c8cf';
  if (!colorByLabel.has(label)) colorByLabel.set(label, PALETTE[colorByLabel.size % PALETTE.length]);
  return colorByLabel.get(label);
}

function imageUrl(value) {
  return typeof value === 'string' && /^https?:\/\//i.test(value.trim()) ? value.trim() : '';
}

function nodeCaption(node) {
  return String(node.name ?? node.title ?? node.id);
}

function nodeProps(node) {
  const skip = new Set(['id', 'labels', 'type', 'from', 'to', 'x', 'y', 'vx', 'vy', 'index', 'fx', 'fy', 'face', 'logo', 'flag', 'image']);
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

function startSimulation(nodes, links, onTick, phys, geometry) {
  const baseCharge = -Math.max(140, 45 * Math.sqrt(nodes.length));
  const sim = forceSimulation(nodes)
    .force('charge', forceManyBody().strength(baseCharge * phys.repulsion / 100))
    .force('link', forceLink(links).id((node) => node.id).distance(phys.linkDist))
    .force('center', forceCenter(0, 0))
    .force('collide', forceCollide((node) => geometry.get(node.id).radius + phys.pad))
    .force('x', forceX(0).strength(phys.gravity / 100))
    .force('y', forceY(0).strength(phys.gravity / 100))
    .alphaDecay(nodes.length > 60 ? 0.012 : 0.0228)
    .on('tick', onTick);
  sim.baseCharge = baseCharge;
  return sim;
}

export function stopSim(container) {
  container._collapse?.();
  container._collapse = null;
  container._resize?.disconnect();
  container._resize = null;
  container._closePreview?.();
  container._closePreview = null;
  if (container._sim) {
    container._sim.stop();
    container._sim = null;
  }
  if (container._float) {
    cancelAnimationFrame(container._float);
    container._float = null;
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
// null means the search returned nothing, so every node and edge is inactive.
// An empty set draws the whole graph. A non-empty set keeps those nodes
// bright and fades the rest.
function graphSignature(graph) {
  const nodes = graph.nodes.map((node) => node.id).sort((a, b) => a - b).join(',');
  const rels = graph.rels.map((rel) => `${rel.id}:${rel.from}:${rel.to}:${rel.type}`).sort().join(',');
  return `${nodes}|${rels}`;
}

export function renderGraph(container, graph, activeArg = new Set(), actions = null, view = {}, types = []) {
  container._actions = actions;
  const signature = graphSignature(graph) + JSON.stringify([view, types, graph.nodes.map((n) => types.flatMap((t) => t.fields.filter((f) => f.kind === "prop").map((f) => n[f.name])))]);
  if (container._graph && container._graph.signature === signature) {
    container._graph.setActive(activeArg);
    return;
  }
  const previous = container._graph?.capture();
  // A re-render while expanded (new data, a tour step) stays expanded.
  const expanded = container.classList.contains('graph-expanded');
  stopSim(container);
  container._graph = null;
  if (!graph.nodes.length) {
    container.innerHTML = '<div class="empty">no nodes yet</div>';
    return;
  }
  let active = activeArg;
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  const configs = new Map(graph.nodes.map((node) => [node.id, node.labels.map((label) => view.nodes?.[label]).find(Boolean) || {}]));
  const geometry = new Map(graph.nodes.map((node) => [node.id, nodeGeometry(configs.get(node.id))]));
  const contents = new Map(graph.nodes.map((node) => [node.id, nodeContent(node, types, graph)]));
  let selected = null;
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
  // The drawable area in SVG units: the 800×480 box, widened or deepened to
  // the element's shape and centred on it, so the closed pane draws exactly as
  // before and a larger element (the expanded modal) has more room, not bars.
  const area = { x: 0, y: 0, w: 800, h: 480 };
  function sizeArea() {
    const { width, height } = svg.getBoundingClientRect();
    if (!width || !height) return;
    area.w = Math.max(800, 480 * width / height);
    area.h = Math.max(480, 800 * height / width);
    area.x = (800 - area.w) / 2;
    area.y = (480 - area.h) / 2;
    svg.setAttribute('viewBox', `${area.x} ${area.y} ${area.w} ${area.h}`);
  }
  const apply = () => { vp.setAttribute('transform', `translate(${state.tx},${state.ty}) scale(${state.scale})`); };
  const lit = (node) => active != null && (active.size === 0 || active.has(nodeCaption(node)));
  const marked = () => active != null && active.size > 0;

  function fit() {
    const xs = graph.nodes.map((node) => node.x);
    const ys = graph.nodes.map((node) => node.y);
    if (!xs.length || xs.some((value) => !Number.isFinite(value))) return;
    const minX = Math.min(...xs) - 70, maxX = Math.max(...xs) + 70;
    const minY = Math.min(...ys) - 70, maxY = Math.max(...ys) + 70;
    const w = maxX - minX || 1, h = maxY - minY || 1;
    state.scale = Math.min(area.w / w, area.h / h, 1.4);
    state.tx = area.x + (area.w - w * state.scale) / 2 - minX * state.scale;
    state.ty = area.y + (area.h - h * state.scale) / 2 - minY * state.scale;
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
    const hit = document.createElementNS(NS, 'path');
    hit.setAttribute('fill', 'none');
    hit.setAttribute('stroke', 'transparent');
    hit.setAttribute('stroke-width', '12');
    hit.setAttribute('pointer-events', 'stroke');
    hit.dataset.rel = String(rel.id);
    hit.style.cursor = 'pointer';
    vp.appendChild(hit);
    vp.appendChild(path);
    const label = document.createElementNS(NS, 'text');
    label.setAttribute('class', 'rlab');
    label.setAttribute('font-size', '10');
    label.setAttribute('fill', '#77777f');
    label.setAttribute('text-anchor', 'middle');
    label.textContent = rel.type;
    if (graph.rels.length > 100) label.style.display = 'none';
    vp.appendChild(label);
    label.dataset.rel = String(rel.id);
    label.style.cursor = 'pointer';
    edges.set(rel.id, { path, hit, label, rel, box: label.getBBox() });
  }

  function drift(node) {
    if (!Number.isFinite(node.x)) return null;
    if (node.fx != null) return { x: node.x, y: node.y };
    const t = performance.now() / 1000;
    const phase = node.id * 0.85;
    return {
      x: node.x + Math.sin(t * 0.7 + phase) * 3.2,
      y: node.y + Math.cos(t * 0.5 + phase * 1.4) * 4.4,
    };
  }

  const intersects = (a, b) => a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y;
  const inView = (box) => intersects(box, {
    x: (area.x - state.tx) / state.scale, y: (area.y - state.ty) / state.scale,
    width: area.w / state.scale, height: area.h / state.scale,
  });

  function placeLabel(drawn, pointAt, obstacles) {
    if (drawn.label.style.display === 'none') return;
    const { box, label } = drawn;
    const baseline = -box.y - box.height / 2;
    const at = (point) => ({ x: point.x + box.x, y: point.y + baseline + box.y, width: box.width, height: box.height });
    const middle = pointAt(0.5);
    const set = (point) => {
      label.setAttribute('x', point.x);
      label.setAttribute('y', point.y + baseline);
    };
    set(middle);
    // Text metrics are cached at creation. Offscreen labels do no collision work.
    if (!inView(at(middle))) return;
    const candidates = [0.5, 0.35, 0.65, 0.25, 0.75].map(pointAt);
    const clear = (point) => !obstacles.some((obstacle) => intersects(at(point), obstacle));
    for (const point of candidates) {
      if (clear(point)) { set(point); return; }
    }
    // Use the local tangent of this edge, including curves and self loops.
    // Bounded offsets keep the label close to its own edge in crowded graphs.
    for (const distance of [12, -12, 24, -24, 36, -36, 48, -48]) {
      for (const point of candidates) {
        const length = Math.hypot(point.dx, point.dy) || 1;
        const offset = { x: point.x - point.dy / length * distance, y: point.y + point.dx / length * distance };
        if (clear(offset)) { set(offset); return; }
      }
    }
  }

  function drawEdge(rel, positions, obstacles) {
    const from = byId.get(rel.from);
    const to = byId.get(rel.to);
    const a = positions.get(rel.from);
    const b = positions.get(rel.to);
    if (!a || !b) return;
    if (rel.from === rel.to) {
      const shape = geometry.get(from.id);
      const start = shape.outline(-1, -1), end = shape.outline(1, -1);
      const reach = shape.radius * 2;
      const drawn = edges.get(rel.id);
      const d = `M ${a.x + start.x} ${a.y + start.y} C ${a.x - reach} ${a.y - reach * 2} ${a.x + reach} ${a.y - reach * 2} ${a.x + end.x} ${a.y + end.y}`;
      drawn.path.setAttribute('d', d);
      drawn.hit.setAttribute('d', d);
      placeLabel(drawn, (t) => {
        const u = 1 - t;
        return {
          x: a.x + u ** 3 * start.x - 3 * u * u * t * reach + 3 * u * t * t * reach + t ** 3 * end.x,
          y: a.y + u ** 3 * start.y - 6 * u * t * reach + t ** 3 * end.y,
          dx: 3 * u * u * (-reach - start.x) + 12 * u * t * reach + 3 * t * t * (end.x - reach),
          dy: 3 * u * u * (-2 * reach - start.y) + 3 * t * t * (end.y + 2 * reach),
        };
      }, obstacles);
      dim(drawn.path, lit(from), 0.2);
      dim(drawn.label, lit(from), 0.2);
      return;
    }
    const dx = b.x - a.x, dy = b.y - a.y;
    // Perpendicular is fixed for the node pair, so an edge in the opposite
    // direction does not fold back onto the same curve.
    const canonX = rel.from < rel.to ? dx : -dx;
    const canonY = rel.from < rel.to ? dy : -dy;
    const canon = Math.hypot(canonX, canonY) || 1;
    const px = -canonY / canon;
    const py = canonX / canon;
    const lane = Math.max(-5, Math.min(5, lanes.get(rel.id) || 0));
    const bow = lane === 0 ? 18 : lane * 46;
    const cx = (a.x + b.x) / 2 + px * bow;
    const cy = (a.y + b.y) / 2 + py * bow;
    const drawn = edges.get(rel.id);
    const start = geometry.get(from.id).outline(cx - a.x, cy - a.y);
    const end = geometry.get(to.id).outline(cx - b.x, cy - b.y);
    const pathD = `M ${a.x + start.x} ${a.y + start.y} Q ${cx} ${cy} ${b.x + end.x} ${b.y + end.y}`;
    drawn.path.setAttribute('d', pathD);
    drawn.hit.setAttribute('d', pathD);
    placeLabel(drawn, (t) => {
      const u = 1 - t, sx = a.x + start.x, sy = a.y + start.y, ex = b.x + end.x, ey = b.y + end.y;
      return {
        x: u * u * sx + 2 * u * t * cx + t * t * ex,
        y: u * u * sy + 2 * u * t * cy + t * t * ey,
        dx: 2 * u * (cx - sx) + 2 * t * (ex - cx),
        dy: 2 * u * (cy - sy) + 2 * t * (ey - cy),
      };
    }, obstacles);
    dim(drawn.path, lit(from) && lit(to), 0.2);
    dim(drawn.label, lit(from) && lit(to), 0.2);
  }

  function dim(el, on, opacity) {
    el.setAttribute('opacity', on ? '1' : String(opacity));
    el.style.filter = on ? '' : 'blur(1.4px)';
  }

  const circles = new Map();
  for (const node of graph.nodes) {
    const g = document.createElementNS(NS, 'g');
    g.dataset.node = String(node.id);
    g.style.cursor = 'pointer';
    const on = lit(node);
    const config = configs.get(node.id);
    const shape = geometry.get(node.id);
    const picture = config.image ? imageUrl(node[config.image]) : '';
    g.dataset.shape = shape.page ? 'document' : 'circle';
    g.dataset.size = String(config.size || 1);
    g.setAttribute('tabindex', '0');
    g.setAttribute('role', 'button');
    g.setAttribute('aria-label', `${contents.get(node.id).heading}. Space to preview`);
    g.setAttribute('aria-pressed', 'false');
    const visual = drawNode(g, svg.querySelector('defs'), shape, picture, labelColor(node.labels[0]), contents.get(node.id));
    const plate = visual.plate;
    g.addEventListener('keydown', (event) => {
      if (event.code === 'Space') {
        event.preventDefault(); event.stopPropagation();
        select(node.id);
        container._closePreview = previewNode(contents.get(node.id), g);
      } else if (event.key === 'Enter') { event.preventDefault(); select(node.id); }
    });
    const text = document.createElementNS(NS, 'text');
    text.setAttribute('class', 'cap');
    text.setAttribute('font-size', '11.5');
    text.setAttribute('text-anchor', 'middle');
    text.setAttribute('dy', shape.height + 17);
    text.setAttribute('fill', 'var(--ink)');
    const caption = nodeCaption(node);
    text.textContent = caption.length > 22 ? caption.slice(0, 21) + '…' : caption;
    g.appendChild(text);
    dim(g, on, 0.28);
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
    circles.set(node.id, { g, node, plate, visual, captionBox: text.getBBox() });
  }

  function select(id) {
    selected = id;
    for (const [key, { g }] of circles) g.setAttribute('aria-pressed', String(key === id));
    circles.get(id)?.g.focus();
    paint();
  }

  function paint() {
    for (const { g, node, plate } of circles.values()) {
      const on = lit(node);
      dim(g, on, 0.28);
      plate.setAttribute('stroke', node.id === selected ? 'var(--accent)' : 'var(--strongRule)');
      plate.setAttribute('stroke-width', node.id === selected || (on && marked()) ? '2.5' : '1');
    }
    for (const rel of graph.rels) {
      const a = byId.get(rel.from);
      const b = byId.get(rel.to);
      const drawn = edges.get(rel.id);
      if (!a || !b || !drawn) continue;
      const on = lit(a) && lit(b);
      dim(drawn.path, on, 0.2);
      dim(drawn.label, on, 0.2);
    }
  }

  function place() {
    const positions = new Map();
    const obstacles = [];
    const block = (box) => {
      // Include the caption's painted halo and a little breathing room.
      const padded = { x: box.x - 5, y: box.y - 5, width: box.width + 10, height: box.height + 10 };
      if (inView(padded)) obstacles.push(padded);
    };
    for (const node of graph.nodes) {
      const entry = circles.get(node.id);
      const at = entry && drift(node);
      if (at) {
        positions.set(node.id, at);
        entry.g.setAttribute('transform', `translate(${at.x},${at.y})`);
        const shape = geometry.get(node.id);
        block({ x: at.x - shape.width, y: at.y - shape.height, width: shape.width * 2, height: shape.height * 2 });
        const box = entry.captionBox;
        block({ x: at.x + box.x, y: at.y + box.y, width: box.width, height: box.height });
        const x = at.x * state.scale + state.tx, y = at.y * state.scale + state.ty;
        const visible = x + shape.width * state.scale >= area.x && x - shape.width * state.scale <= area.x + area.w && y + shape.height * state.scale >= area.y && y - shape.height * state.scale <= area.y + area.h;
        entry.visual.update(visible, state.scale);
      }
    }
    for (const rel of graph.rels) drawEdge(rel, positions, obstacles);
  }

  function onTick() {
    if (!state.fitted) fit();
    place();
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
  const sim = startSimulation(graph.nodes, links, onTick, phys, geometry);
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
  hint.textContent = 'Scroll to zoom. Drag to navigate. Select a node, Space to preview.';
  const expand = document.createElement('button');
  expand.type = 'button';
  expand.className = 'graph-expand';
  expand.innerHTML = '<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><path d="M2 6V2h4M10 2h4v4M14 10v4h-4M6 14H2v-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  wrap.appendChild(hint);
  wrap.appendChild(expand);
  wrap.appendChild(gear);
  wrap.appendChild(panel);
  const zoom = document.createElement('div');
  zoom.className = 'graph-zoom';
  zoom.innerHTML = `<button type="button" data-zoom="in" title="zoom in">+</button><button type="button" data-zoom="out" title="zoom out">−</button>`;
  wrap.appendChild(zoom);
  function zoomBy(factor) {
    userInteracted = true;
    const before = state.scale;
    state.scale = Math.min(4, Math.max(0.1, state.scale * factor));
    state.tx = 400 - (400 - state.tx) * state.scale / before;
    state.ty = 240 - (240 - state.ty) * state.scale / before;
    apply();
  }
  zoom.querySelector('[data-zoom="in"]').onclick = () => zoomBy(1.12);
  zoom.querySelector('[data-zoom="out"]').onclick = () => zoomBy(1 / 1.12);
  gear.onclick = () => panel.classList.toggle('open');
  const format = { repulsion: (value) => value + '%', linkDist: (value) => value, pad: (value) => value, gravity: (value) => value + '%' };
  function applyPhys() {
    sim.force('charge').strength(sim.baseCharge * phys.repulsion / 100);
    sim.force('link').distance(phys.linkDist);
    sim.force('collide').radius((node) => geometry.get(node.id).radius + phys.pad);
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

  // Expanded, the container is a near-full-viewport modal. Nothing is
  // re-rendered, so the simulation, zoom and selection carry over; the view
  // re-fits to the new size (below) unless the reader has zoomed or panned.
  function setExpanded(on, { focus = true } = {}) {
    container.classList.toggle('graph-expanded', on);
    expand.setAttribute('aria-pressed', String(on));
    expand.setAttribute('aria-label', on ? 'Close expanded graph' : 'Expand graph');
    expand.title = on ? 'close (Esc)' : 'expand graph';
    if (on) {
      container.setAttribute('role', 'dialog');
      container.setAttribute('aria-modal', 'true');
      container.setAttribute('aria-label', 'Graph, expanded');
      document.addEventListener('keydown', onEscape, true);
      container._collapse = () => setExpanded(false, { focus: false });
    } else {
      for (const name of ['role', 'aria-modal', 'aria-label']) container.removeAttribute(name);
      document.removeEventListener('keydown', onEscape, true);
      container._collapse = null;
    }
    if (focus) expand.focus();
  }
  // Capture phase, so an open context menu or node preview takes the first
  // Escape and the modal the next one.
  function onEscape(event) {
    if (event.key !== 'Escape' || document.querySelector('.graph-menu, dialog[open]')) return;
    event.preventDefault();
    setExpanded(false);
  }
  expand.onclick = () => setExpanded(!container.classList.contains('graph-expanded'));
  setExpanded(expanded, { focus: false });

  sizeArea();
  // Only a real change of size re-fits: the observer's first call, and a
  // re-render that restored the previous view, keep the view they have.
  container._resize = new ResizeObserver(() => {
    const { w, h } = area;
    sizeArea();
    if (area.w === w && area.h === h) return;
    if (!userInteracted) fit();
    place();
  });
  container._resize.observe(svg);

  let drag = null;
  if (container._onMenu) container.removeEventListener('contextmenu', container._onMenu);
  container._onMenu = (event) => {
    event.preventDefault();
    tip.style.display = 'none';
    const nodeEl = event.target.closest && event.target.closest('g[data-node]');
    if (nodeEl) {
      const entry = circles.get(Number(nodeEl.dataset.node));
      if (entry) container._actions?.onNode?.(entry.node, event.clientX, event.clientY);
      return;
    }
    const edgeEl = event.target.closest && event.target.closest('[data-rel]');
    if (edgeEl) {
      const drawn = edges.get(Number(edgeEl.dataset.rel));
      if (drawn) container._actions?.onEdge?.(drawn.rel, event.clientX, event.clientY);
    }
  };
  container.addEventListener('contextmenu', container._onMenu);

  svg.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    const nodeEl = event.target.closest && event.target.closest('g[data-node]');
    svg.setPointerCapture(event.pointerId);
    const entry = nodeEl && [...circles.values()].find((item) => item.g === nodeEl);
    if (entry) {
      userInteracted = true;
      drag = { node: entry.node, sx: event.clientX, sy: event.clientY };
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
  svg.addEventListener('pointerup', (event) => {
    if (drag && drag.node) {
      if (Math.hypot(event.clientX - drag.sx, event.clientY - drag.sy) < 5) { select(drag.node.id); container._actions?.onInspect?.(drag.node); }
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
    const before = state.scale;
    state.scale = Math.min(4, Math.max(0.1, state.scale * factor));
    state.tx = 400 - (400 - state.tx) * state.scale / before;
    state.ty = 240 - (240 - state.ty) * state.scale / before;
    apply();
  }, { passive: false });

  function toSvg(event) {
    const rect = svg.getBoundingClientRect();
    const x = area.x + ((event.clientX - rect.left) / rect.width) * area.w;
    const y = area.y + ((event.clientY - rect.top) / rect.height) * area.h;
    return { x: (x - state.tx) / state.scale, y: (y - state.ty) / state.scale };
  }

  function float() {
    container._float = requestAnimationFrame(float);
    place();
  }
  container._float = requestAnimationFrame(float);

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
