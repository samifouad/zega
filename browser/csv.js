// CSV import for the explorer. The schema text is what the user edits.
// Dragging a column inserts a field or a new type. A column is covered when
// its name shows up in that schema.

const PRESETS = {
  Hockey: `Name,Team,Position,Country,Salary
Connor McDavid,Oilers,C,Canada,12500000
Leon Draisaitl,Oilers,C,Germany,14000000
Auston Matthews,Maple Leafs,C,United States,13250000
Nathan MacKinnon,Avalanche,C,Canada,12604000
Cale Makar,Avalanche,D,Canada,9000000
Nikita Kucherov,Lightning,RW,Russia,9500000
Alex Ovechkin,Capitals,LW,Russia,4250000
Mitch Marner,Golden Knights,RW,Canada,12000000`,
  Pokemon: `Name,Type 1,Type 2,Total,HP,Attack,Defense,Sp. Atk,Sp. Def,Speed,Generation,Legendary
Bulbasaur,Grass,Poison,318,45,49,49,65,65,45,1,False
Ivysaur,Grass,Poison,405,60,62,63,80,80,60,1,False
Charmander,Fire,,309,39,52,43,60,50,65,1,False
Charizard,Fire,Flying,534,78,84,78,109,85,100,1,False
Squirtle,Water,,314,44,48,65,50,64,43,1,False
Pikachu,Electric,,320,35,55,40,50,50,90,1,False
Eevee,Normal,,325,55,55,50,45,65,55,1,False
Mewtwo,Psychic,,680,106,110,90,154,90,130,1,True
Chikorita,Grass,,318,45,49,65,49,65,45,2,False
Cyndaquil,Fire,,309,39,52,43,60,50,65,2,False
Totodile,Water,,314,50,65,64,44,48,43,2,False`,
  Movies: `Actor,Film,Year,Role
Tom Hanks,Forrest Gump,1994,Forrest Gump
Tom Hanks,Cast Away,2000,Chuck Noland
Meg Ryan,Sleepless in Seattle,1993,Annie Reed
Meg Ryan,You've Got Mail,1998,Kathleen Kelly
Robert Zemeckis,Forrest Gump,1994,Director
Robert Zemeckis,Cast Away,2000,Director`,
};

const ROW_CAP = 150;

export function openCsv({ run, setSchema, setQuery }) {
  closeCsv();
  const root = document.createElement('div');
  root.id = 'csv-modal';
  root.innerHTML = `
    <div class="csv-dialog" role="dialog" aria-label="CSV import">
      <header>
        <strong>CSV</strong>
        <span class="csv-hint">Drop a column on a type to add it there. Drop it anywhere else in the schema to create a type.</span>
        <button id="csv-close" type="button">close</button>
      </header>
      <div class="csv-body">
        <aside>
          <button type="button" id="csv-file-btn">Open a CSV…</button>
          <input id="csv-file" type="file" accept=".csv,text/csv,text/plain">
          ${Object.keys(PRESETS).map((name) => `<button type="button" class="csv-preset" data-preset="${name}">${name}</button>`).join('')}
        </aside>
        <div class="csv-main">
          <section class="csv-schema-pane">
            <div class="csv-schema-frame">
              <textarea id="csv-schema" spellcheck="false" placeholder="type Pokemon {\n  name: String\n}"></textarea>
              <div id="csv-drop-hl" hidden></div>
              <div id="csv-drop-hint" hidden></div>
            </div>
          </section>
          <section class="csv-table-pane">
            <div id="csv-table-wrap"><p class="csv-empty">Choose a sample, or open a CSV.</p></div>
          </section>
        </div>
      </div>
      <footer>
        <span id="csv-status"></span>
        <button id="csv-import" class="primary" type="button" disabled>Import</button>
      </footer>
    </div>`;
  document.body.appendChild(root);

  const schemaEl = root.querySelector('#csv-schema');
  const tableWrap = root.querySelector('#csv-table-wrap');
  const status = root.querySelector('#csv-status');
  const importBtn = root.querySelector('#csv-import');
  let headers = [];
  let rows = [];

  const refresh = () => {
    importBtn.disabled = !/type\s+[A-Za-z_]/.test(schemaEl.value) || !rows.length;
    paintTable();
  };

  const load = (text, label) => {
    const parsed = parseCsv(text);
    headers = parsed.headers;
    rows = parsed.rows.slice(0, ROW_CAP);
    status.textContent = rows.length < parsed.rows.length
      ? `${label}: first ${ROW_CAP} of ${parsed.rows.length} rows`
      : `${label}: ${rows.length} rows`;
    if (!schemaEl.value.trim()) schemaEl.value = '';
    refresh();
  };

  root.querySelectorAll('.csv-preset').forEach((button) => {
    button.onclick = () => {
      root.querySelectorAll('.csv-preset').forEach((b) => b.classList.toggle('active', b === button));
      schemaEl.value = '';
      load(PRESETS[button.dataset.preset], button.dataset.preset);
    };
  });
  root.querySelector('#csv-file-btn').onclick = () => root.querySelector('#csv-file').click();
  root.querySelector('#csv-file').onchange = (event) => {
    const file = event.target.files[0];
    if (!file) return;
    file.text().then((text) => {
      schemaEl.value = '';
      load(text, file.name);
    });
  };
  root.querySelector('#csv-close').onclick = closeCsv;
  root.addEventListener('click', (event) => { if (event.target === root) closeCsv(); });
  root.addEventListener('keydown', (event) => { if (event.key === 'Escape') closeCsv(); });
  schemaEl.addEventListener('input', refresh);

  const highlight = root.querySelector('#csv-drop-hl');
  const hint = root.querySelector('#csv-drop-hint');

  const showDrop = (header, block) => {
    if (!header) {
      highlight.hidden = true;
      hint.hidden = true;
      return;
    }
    if (block) {
      const style = getComputedStyle(schemaEl);
      const lineHeight = parseFloat(style.lineHeight) || 20;
      const padTop = parseFloat(style.paddingTop) || 0;
      const border = parseFloat(style.borderTopWidth) || 0;
      highlight.style.top = `${border + padTop + block.start * lineHeight - schemaEl.scrollTop}px`;
      highlight.style.height = `${(block.end - block.start + 1) * lineHeight}px`;
      highlight.hidden = false;
      const edge = existingTypeFor(header, schemaEl.value);
      hint.textContent = edge && edge !== block.name
        ? `Drop to add ${header} → ${edge} to node ${block.name}`
        : `Drop to add ${header} to node ${block.name}`;
    } else {
      highlight.hidden = true;
      hint.textContent = `Drop to create node ${typeNameFrom(header)}`;
    }
    hint.hidden = false;
  };
  const hideDrop = () => {
    highlight.hidden = true;
    hint.hidden = true;
  };

  schemaEl.addEventListener('dragover', (event) => {
    event.preventDefault();
    showDrop(draggingHeader(), typeAtPoint(schemaEl, event));
  });
  schemaEl.addEventListener('dragleave', (event) => {
    const rect = schemaEl.getBoundingClientRect();
    const inside = event.clientX >= rect.left && event.clientX <= rect.right
      && event.clientY >= rect.top && event.clientY <= rect.bottom;
    if (!inside) hideDrop();
  });
  schemaEl.addEventListener('drop', (event) => {
    event.preventDefault();
    hideDrop();
    const header = event.dataTransfer.getData('text/plain') || dragHeader;
    if (!header) return;
    const block = typeAtPoint(schemaEl, event);
    if (block) addField(schemaEl, block, header, columnValues(header));
    else addType(schemaEl, header);
    refresh();
  });
  root.addEventListener('dragend', hideDrop);

  importBtn.onclick = () => {
    const built = buildImport(schemaEl.value, headers, rows);
    if (built.error) {
      status.textContent = built.error;
      return;
    }
    setSchema(built.schema);
    for (const mutation of built.mutations) {
      const value = run(mutation);
      if (value == null) {
        status.textContent = 'Import stopped on a row that did not apply. The output pane has the reason.';
        return;
      }
    }
    setQuery(built.query);
    run(built.query);
    closeCsv();
  };

  function columnValues(header) {
    const index = headers.indexOf(header);
    return rows.map((row) => row[index] ?? '');
  }
  function paintTable() {
    if (!headers.length) {
      tableWrap.innerHTML = '<p class="csv-empty">Choose a sample, or open a CSV.</p>';
      return;
    }
    const schema = schemaEl.value;
    const head = headers.map((header) => {
      const on = covered(header, schema);
      return `<th draggable="true" data-header="${escapeAttr(header)}" class="${on ? 'covered' : ''}"><span class="csv-mark">${on ? '✓' : ''}</span>${escapeHtml(header)}</th>`;
    }).join('');
    const body = rows.slice(0, 40).map((row) => `<tr>${headers.map((_, i) => `<td>${escapeHtml(row[i] ?? '')}</td>`).join('')}</tr>`).join('');
    const more = rows.length > 40 ? `<p class="csv-empty">Showing 40 of ${rows.length} rows.</p>` : '';
    tableWrap.innerHTML = `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>${more}`;
    tableWrap.querySelectorAll('th').forEach((th) => {
      th.addEventListener('dragstart', (event) => {
        dragHeader = th.dataset.header;
        event.dataTransfer.setData('text/plain', th.dataset.header);
        event.dataTransfer.effectAllowed = 'copy';
        th.classList.add('dragging');
      });
      th.addEventListener('dragend', () => {
        dragHeader = '';
        th.classList.remove('dragging');
      });
    });
  }
}

function closeCsv() {
  document.getElementById('csv-modal')?.remove();
}

let dragHeader = '';
function draggingHeader() {
  return dragHeader;
}

function covered(header, schema) {
  const id = ident(header);
  const typeName = typeNameFrom(header);
  return new RegExp(`\\b${id}\\b`).test(schema) || new RegExp(`\\btype\\s+${typeName}\\b`).test(schema);
}

function ident(header) {
  const parts = header.replace(/\./g, ' ').split(/[^A-Za-z0-9]+/).filter(Boolean);
  if (!parts.length) return 'col';
  const [first, ...rest] = parts;
  return first.toLowerCase() + rest.map((part) => part.charAt(0).toUpperCase() + part.slice(1).toLowerCase()).join('');
}

function typeNameFrom(header) {
  const stripped = header.replace(/\s*\d+\s*$/, '').trim() || header;
  const id = ident(stripped);
  return id.charAt(0).toUpperCase() + id.slice(1);
}

function guessType(values) {
  const present = values.map((value) => value.trim()).filter(Boolean);
  if (!present.length) return 'String';
  if (present.every((value) => /^(true|false)$/i.test(value))) return 'Bool';
  if (present.every((value) => /^-?\d+$/.test(value))) return 'Int';
  if (present.every((value) => /^-?\d+\.\d+$/.test(value))) return 'Float';
  return 'String';
}

function existingTypeFor(header, schema) {
  if (ident(header) === 'name') return null;
  const typeName = typeNameFrom(header);
  return new RegExp(`\\btype\\s+${typeName}\\b`).test(schema) ? typeName : null;
}

function typeBlocks(src) {
  const lines = src.split('\n');
  const blocks = [];
  let current = null;
  lines.forEach((text, index) => {
    const open = text.match(/^\s*type\s+([A-Za-z_][\w]*)\s*\{/);
    if (open) current = { name: open[1], start: index, end: index };
    if (current && text.includes('}')) {
      current.end = index;
      blocks.push(current);
      current = null;
    }
  });
  return blocks;
}

function typeAtPoint(textarea, event) {
  const style = getComputedStyle(textarea);
  const lineHeight = parseFloat(style.lineHeight) || 20;
  const rect = textarea.getBoundingClientRect();
  const y = event.clientY - rect.top + textarea.scrollTop - (parseFloat(style.paddingTop) || 0);
  const line = Math.max(0, Math.floor(y / lineHeight));
  return typeBlocks(textarea.value).find((block) => line >= block.start && line <= block.end) || null;
}

function addType(textarea, header) {
  const typeName = typeNameFrom(header);
  if (new RegExp(`\\btype\\s+${typeName}\\b`).test(textarea.value)) return;
  const block = `type ${typeName} {\n  name: String\n}\n`;
  textarea.value = textarea.value.trim() ? `${textarea.value.trim()}\n\n${block}` : block;
}

function addField(textarea, block, header, values) {
  const field = ident(header);
  const lines = textarea.value.split('\n');
  const body = lines.slice(block.start, block.end + 1).join('\n');
  if (new RegExp(`\\b${field}\\b`).test(body)) return;
  const edge = existingTypeFor(header, textarea.value);
  const line = edge ? `  ${field} -> ${edge}` : `  ${field}: ${guessType(values)}`;
  lines.splice(block.end, 0, line);
  textarea.value = lines.join('\n');
}

function parseSchema(src) {
  const types = [];
  const re = /type\s+([A-Za-z_][\w]*)\s*\{([^}]*)\}/g;
  let match = re.exec(src);
  while (match) {
    const fields = [];
    for (const raw of match[2].split('\n')) {
      const line = raw.trim();
      const edge = line.match(/^([A-Za-z_][\w]*)\??\s*(->|<-)\s*([A-Za-z_][\w]*)/);
      const prop = line.match(/^([A-Za-z_][\w]*)\??\s*:\s*([A-Za-z_][\w]*)/);
      if (edge) fields.push({ name: edge[1], edge: true, dir: edge[2], target: edge[3], zql: edge[3] });
      else if (prop) fields.push({ name: prop[1], edge: false, zql: prop[2] });
    }
    types.push({ name: match[1], fields });
    match = re.exec(src);
  }
  return types;
}

function buildImport(schema, headers, rows) {
  const types = parseSchema(schema);
  if (!types.length) return { error: 'The schema has no types yet.' };
  const score = (type) => type.fields.filter((field) => headers.some((header) => ident(header) === field.name)).length;
  const fact = types.slice().sort((a, b) => score(b) - score(a))[0];
  if (!score(fact)) return { error: 'None of the columns are fields on a type yet. Drag a column onto a type.' };

  const column = (field) => headers.find((header) => ident(header) === field.name);
  const targets = new Map();
  for (const field of fact.fields) {
    if (!field.edge) continue;
    const header = column(field);
    if (!header) continue;
    const index = headers.indexOf(header);
    const values = targets.get(field.target) || new Set();
    for (const row of rows) {
      const value = (row[index] ?? '').trim();
      if (value) values.add(value);
    }
    targets.set(field.target, values);
  }

  const mutations = [];
  for (const [typeName, values] of targets) {
    for (const value of values) {
      mutations.push(`mutation {\n  ${typeName}(name: ${quote(value)}) { name }\n}`);
    }
  }
  const indexOf = Object.fromEntries(headers.map((header, index) => [header, index]));
  for (const row of rows) {
    const props = [];
    const links = [];
    for (const field of fact.fields) {
      const header = column(field);
      if (!header) continue;
      const value = (row[indexOf[header]] ?? '').trim();
      if (!value) continue;
      if (field.edge) links.push(`    ${field.name} ${field.dir} link ${field.target}(name: ${quote(value)}) { name }`);
      else props.push(`${field.name}: ${literal(value, field.zql)}`);
    }
    if (!props.length && !links.length) continue;
    const head = props.length ? `(${props.join(' && ')})` : '';
    const body = links.length ? ` {\n${links.join('\n')}\n  }` : ' { name }';
    mutations.push(`mutation {\n  ${fact.name}${head}${body}\n}`);
  }
  const shown = fact.fields.filter((field) => field.edge).slice(0, 3)
    .map((field) => `    ${field.name} ${field.dir} ${field.target} { name }`)
    .join('\n');
  const scalars = fact.fields.filter((field) => !field.edge).slice(0, 4).map((field) => field.name);
  const query = `{\n  ${fact.name} {\n    ${scalars.join('\n    ')}\n${shown}\n  }\n}`;
  return { schema, mutations, query };
}

function literal(value, zql) {
  if (zql === 'Int' || zql === 'Float') return /^-?\d+(\.\d+)?$/.test(value) ? value : '0';
  if (zql === 'Bool') return /^(true|yes|1)$/i.test(value) ? 'true' : 'false';
  return quote(value);
}

function quote(value) {
  return `"${String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

function parseCsv(text) {
  const rows = [];
  let row = [];
  let cell = '';
  let quoted = false;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') { cell += '"'; i += 1; } else quoted = false;
      } else cell += ch;
    } else if (ch === '"') quoted = true;
    else if (ch === ',') { row.push(cell); cell = ''; }
    else if (ch === '\n') { row.push(cell); rows.push(row); row = []; cell = ''; }
    else if (ch !== '\r') cell += ch;
  }
  if (cell.length || row.length) { row.push(cell); rows.push(row); }
  const headers = (rows.shift() || []).map((header) => header.trim());
  return { headers, rows: rows.filter((cells) => cells.some((value) => value.trim())) };
}

function escapeHtml(value) {
  return String(value).replace(/[&<>]/g, (ch) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch]));
}

function escapeAttr(value) {
  return escapeHtml(value).replace(/"/g, '&quot;');
}
