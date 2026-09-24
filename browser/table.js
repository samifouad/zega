import { format_json } from './pkg/zega_wasm.js';
const caption = (node) => String(node.name ?? node.title ?? node.id);
const cellText = (value) => value == null ? '—' : typeof value === 'object' ? format_json(JSON.stringify(value)).trimEnd() : String(value);

export function renderTable(container, graph, types, onNode) {
  const root = document.createElement('div');
  root.className = 'table-view';
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  for (const type of types) {
    const section = document.createElement('section');
    section.dataset.type = type.name;
    const nodes = graph.nodes.filter((node) => node.labels.includes(type.name));
    const heading = document.createElement('h3');
    heading.textContent = `${type.name} · ${nodes.length}`;
    const table = document.createElement('table');
    table.setAttribute('aria-label', type.name);
    const head = table.createTHead().insertRow();
    const body = table.createTBody();
    let sortField = null, direction = 1;
    const relations = (node, field) => graph.rels.filter((rel) => rel.type === field.rel && (field.direction === 'out' ? rel.from : rel.to) === node.id)
      .map((rel) => byId.get(field.direction === 'out' ? rel.to : rel.from)).filter(Boolean);
    const value = (node, field) => field.kind === 'edge' ? relations(node, field).map(caption).join(', ') : node[field.name];
    const compare = (a, b) => a == null ? (b == null ? 0 : 1) : b == null ? -1 : typeof a === 'number' && typeof b === 'number' ? a - b : String(a).localeCompare(String(b), undefined, { numeric: true });
    function draw() {
      body.replaceChildren();
      const ordered = [...nodes].sort((a, b) => sortField ? direction * compare(value(a, sortField), value(b, sortField)) || a.id - b.id : a.id - b.id);
      for (const node of ordered) {
        const row = body.insertRow();
        row.dataset.node = node.id;
        for (const field of type.fields) {
          const cell = row.insertCell();
          if (field.kind === 'edge') {
            for (const target of relations(node, field)) {
              const chip = document.createElement('button');
              chip.className = 'chip rel';
              chip.textContent = caption(target);
              chip.onclick = () => onNode(target);
              cell.append(chip);
            }
          } else {
            cell.textContent = cellText(value(node, field));
            cell.tabIndex = 0;
            cell.onclick = () => onNode(node);
            cell.onkeydown = (event) => { if (event.key === 'Enter') onNode(node); };
          }
        }
      }
      for (let i = 0; i < head.cells.length; i++) head.cells[i].setAttribute('aria-sort', type.fields[i] === sortField ? direction === 1 ? 'ascending' : 'descending' : 'none');
    }
    for (const field of type.fields) {
      const th = document.createElement('th');
      const button = document.createElement('button');
      button.textContent = field.kind === 'edge' ? field.field : field.name;
      button.onclick = () => { direction = sortField === field ? -direction : 1; sortField = field; draw(); };
      th.append(button);
      head.append(th);
    }
    draw();
    section.append(heading, table);
    root.append(section);
  }
  container.replaceChildren(root);
}
