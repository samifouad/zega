const NS = 'http://www.w3.org/2000/svg';
export const MINI_ZOOM = 2;
export const MINI_ROWS = 5;
export const PAGE_PATH = 'M -14 -22 H 9 L 18 -13 V 18 Q 18 22 14 22 H -14 Q -18 22 -18 18 V -18 Q -18 -22 -14 -22 Z';
export const FOLD_PATH = 'M 9 -22 L 18 -13 H 9 Z';
const svg = (tag, attributes = {}) => {
  const element = document.createElementNS(NS, tag);
  for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
  return element;
};

export function readable(value) {
  if (value == null) return '—';
  if (Array.isArray(value)) return value.length ? value.map(readable).join('; ') : '(empty)';
  if (typeof value === 'object') return Object.entries(value).map(([key, item]) => `${key}: ${readable(item)}`).join('; ') || '(empty)';
  return String(value);
}

// One content model supplies both Quick Look and the zoomed page.
export function nodeContent(node, types, graph) {
  const type = types.find((type) => node.labels.includes(type.name));
  const heading = type?.fields.some((field) => field.kind === 'prop' && field.name === 'name')
    ? readable(node.name) : `${type?.name || node.labels[0] || 'Node'} #${node.id}`;
  const rows = (type?.fields || []).map((field) => {
    if (field.kind !== 'edge') return [field.name, readable(node[field.name])];
    const targets = graph.rels.filter((rel) => rel.type === field.rel && (field.direction === 'out' ? rel.from : rel.to) === node.id)
      .map((rel) => graph.nodes.find((n) => n.id === (field.direction === 'out' ? rel.to : rel.from))).filter(Boolean);
    return [field.field, targets.map((n) => readable(n.name ?? `${n.labels[0]} #${n.id}`)).join('; ') || '—'];
  });
  return { heading, rows };
}

export function previewNode(content, returnFocus) {
  const dialog = document.createElement('dialog');
  dialog.className = 'node-preview';
  dialog.setAttribute('role', 'dialog');
  dialog.setAttribute('aria-modal', 'true');
  dialog.setAttribute('aria-labelledby', 'node-preview-title');
  const heading = document.createElement('h2');
  heading.id = 'node-preview-title';
  heading.textContent = content.heading;
  const close = document.createElement('button');
  close.type = 'button';
  close.textContent = 'Close preview';
  close.autofocus = true;
  close.onclick = () => dialog.close();
  const table = document.createElement('table');
  const header = table.createTHead().insertRow();
  for (const label of ['Field', 'Value']) {
    const th = document.createElement('th'); th.scope = 'col'; th.textContent = label; header.append(th);
  }
  const body = table.createTBody();
  for (const [field, value] of content.rows) {
    const row = body.insertRow();
    const th = document.createElement('th'); th.scope = 'row'; th.textContent = field; row.append(th);
    row.insertCell().textContent = value;
  }
  dialog.append(close, heading, table);
  dialog.addEventListener('keydown', (event) => {
    if (event.code === 'Space') { event.preventDefault(); event.stopPropagation(); dialog.close(); }
    // The native modal makes the rest of the page inert. There is one control.
    if (event.key === 'Tab') { event.preventDefault(); close.focus(); }
  });
  dialog.addEventListener('click', (event) => {
    const rect = dialog.getBoundingClientRect();
    if (event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) dialog.close();
  });
  dialog.addEventListener('close', () => { dialog.remove(); returnFocus?.focus(); }, { once: true });
  document.body.append(dialog);
  dialog.showModal();
  return () => { dialog.close(); dialog.remove(); };
}

export function nodeGeometry(config = {}) {
  const scale = ({ 1: 1, 2: 1.5, 3: 2 })[config.size || 1];
  const page = config.shape === 'document';
  const width = page ? 18 : 22, height = 22;
  const contains = (x, y) => {
    if (!page) return x * x + y * y <= 22 * 22;
    if (Math.abs(x) > 18 || Math.abs(y) > 22 || x - y > 31) return false;
    // The other three page corners follow the quadratic path's rounded corner.
    if (Math.abs(x) > 14 && Math.abs(y) > 18 && !(x > 0 && y < 0)) {
      const a = (Math.abs(x) - 14) / 4, b = (Math.abs(y) - 18) / 4;
      return Math.sqrt(1 - a) + Math.sqrt(1 - b) >= 1;
    }
    return true;
  };
  return {
    scale, page, width: width * scale, height: height * scale,
    radius: (page ? Math.hypot(width, height) : 22) * scale,
    outline(dx, dy) {
      const distance = Math.hypot(dx, dy) || 1, ux = dx / distance, uy = dy / distance;
      let low = 0, high = 30;
      for (let i = 0; i < 20; i++) {
        const mid = (low + high) / 2;
        if (contains(ux * mid, uy * mid)) low = mid; else high = mid;
      }
      return { x: ux * low * scale, y: uy * low * scale };
    },
  };
}

let clipSerial = 0;
export function drawNode(g, defs, geometry, picture, color, content) {
  const { page, scale } = geometry;
  const body = svg('g', { class: 'node-body', transform: `scale(${scale})` });
  const plate = page ? svg('path', { d: PAGE_PATH, class: 'node-plate document-page' }) : svg('circle', { r: 22, class: 'node-plate' });
  plate.setAttribute('fill', page ? 'var(--panel)' : color);
  plate.setAttribute('stroke', 'var(--strongRule)');
  body.append(plate);
  const icon = svg('g', { class: 'document-icon', stroke: 'var(--dim)', 'stroke-width': 1.2, 'stroke-linecap': 'round' });
  if (page) {
    icon.append(svg('path', { d: 'M -11 -13 H 2', 'stroke-width': 2 }));
    for (const y of [-5, 0, 5, 10, 15]) icon.append(svg('path', { d: `M -11 ${y} H ${y === 15 ? 3 : 11}` }));
    body.append(icon);
  }
  const clipId = `node-clip-${++clipSerial}`;
  const clip = svg('clipPath', { id: clipId });
  clip.append(page ? svg('path', { d: PAGE_PATH }) : svg('circle', { r: 22 }));
  defs.append(clip);
  const image = svg('image', { class: 'node-image', x: page ? -18 : -22, y: -22, width: page ? 36 : 44, height: 44, 'clip-path': `url(#${clipId})`, preserveAspectRatio: 'xMidYMid slice', visibility: 'hidden' });
  let started = false, loaded = false;
  // Setting href only on visibility starts the request. Geometry never depends on it.
  image.onload = () => { loaded = true; image.setAttribute('visibility', 'visible'); icon.style.display = 'none'; g.dataset.image = 'loaded'; };
  image.onerror = () => { image.remove(); icon.style.display = ''; g.dataset.image = 'failed'; };
  if (picture) { g.dataset.image = 'pending'; body.append(image); }
  let mini = null;
  if (page) body.append(svg('path', { d: FOLD_PATH, class: 'document-fold', fill: 'var(--border)', stroke: 'var(--strongRule)', 'stroke-width': 0.6 }));
  g.append(body);
  const truncate = (value, count) => value.length > count ? value.slice(0, count - 1) + '…' : value;
  return {
    plate,
    update(visible, zoom) {
      if (visible && picture && !started) { started = true; image.setAttribute('href', picture); }
      const showMini = visible && page && zoom >= MINI_ZOOM;
      icon.style.display = showMini || loaded ? 'none' : '';
      if (!showMini) { mini?.remove(); mini = null; return; }
      if (mini) return;
      mini = svg('g', { class: 'document-mini', 'aria-hidden': 'true', 'pointer-events': 'none' });
      mini.append(svg('rect', { x: -16, y: -11, width: 32, height: 30, rx: 1, fill: 'var(--panel)' }));
      const heading = svg('text', { x: -13, y: -6, fill: 'var(--ink)', 'font-size': 3.1, 'font-weight': 700 });
      heading.textContent = truncate(content.heading, 17); mini.append(heading);
      content.rows.slice(0, MINI_ROWS).forEach(([field, value], i) => {
        const y = i * 4.4;
        mini.append(svg('path', { d: `M -13 ${y - 2.8} H 13`, stroke: 'var(--border)', 'stroke-width': 0.3 }));
        for (const [text, x, max] of [[field, -13, 9], [value, -1, 12]]) {
          const cell = svg('text', { x, y, fill: 'var(--ink)', 'font-size': 2.3 });
          cell.textContent = truncate(text, max); mini.append(cell);
        }
      });
      body.append(mini);
    },
  };
}
