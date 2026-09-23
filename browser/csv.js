// CSV import for the explorer. The schema text is what the user edits.
// Dragging a column inserts a field or a new type. A column is covered when
// its name shows up in that schema.

const PRESETS = {
  Hockey: `Name,Team,Position,Country,Salary,Image
Connor McDavid,Oilers,C,Canada,12500000,https://assets.nhle.com/mugs/nhl/latest/8478402.png
Leon Draisaitl,Oilers,C,Germany,14000000,https://assets.nhle.com/mugs/nhl/latest/8477934.png
Auston Matthews,Maple Leafs,C,United States,13250000,https://assets.nhle.com/mugs/nhl/latest/8479318.png
Nathan MacKinnon,Avalanche,C,Canada,12604000,https://assets.nhle.com/mugs/nhl/latest/8477492.png
Cale Makar,Avalanche,D,Canada,9000000,https://assets.nhle.com/mugs/nhl/latest/8480069.png
Nikita Kucherov,Lightning,RW,Russia,9500000,https://assets.nhle.com/mugs/nhl/latest/8476453.png
Alex Ovechkin,Capitals,LW,Russia,4250000,https://assets.nhle.com/mugs/nhl/latest/8471214.png
Mitch Marner,Golden Knights,RW,Canada,12000000,https://assets.nhle.com/mugs/nhl/latest/8478483.png`,
  Pokemon: `Name,Type 1,Type 2,Total,HP,Attack,Defense,Sp. Atk,Sp. Def,Speed,Generation,Legendary,Image
Bulbasaur,Grass,Poison,318,45,49,49,65,65,45,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/1.png
Ivysaur,Grass,Poison,405,60,62,63,80,80,60,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/2.png
Charmander,Fire,,309,39,52,43,60,50,65,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/4.png
Charizard,Fire,Flying,534,78,84,78,109,85,100,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/6.png
Squirtle,Water,,314,44,48,65,50,64,43,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/7.png
Pikachu,Electric,,320,35,55,40,50,50,90,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/25.png
Eevee,Normal,,325,55,55,50,45,65,55,1,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/133.png
Mewtwo,Psychic,,680,106,110,90,154,90,130,1,True,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/150.png
Chikorita,Grass,,318,45,49,65,49,65,45,2,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/152.png
Cyndaquil,Fire,,309,39,52,43,60,50,65,2,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/155.png
Totodile,Water,,314,50,65,64,44,48,43,2,False,https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork/158.png`,
  Movies: `Actor,Film,Year,Role,Image
Tom Hanks,Forrest Gump,1994,Forrest Gump,https://api.dicebear.com/9.x/personas/svg?seed=Tom%20Hanks
Tom Hanks,Cast Away,2000,Chuck Noland,https://api.dicebear.com/9.x/personas/svg?seed=Tom%20Hanks
Meg Ryan,Sleepless in Seattle,1993,Annie Reed,https://api.dicebear.com/9.x/personas/svg?seed=Meg%20Ryan
Meg Ryan,You've Got Mail,1998,Kathleen Kelly,https://api.dicebear.com/9.x/personas/svg?seed=Meg%20Ryan
Robert Zemeckis,Forrest Gump,1994,Director,https://api.dicebear.com/9.x/personas/svg?seed=Robert%20Zemeckis
Robert Zemeckis,Cast Away,2000,Director,https://api.dicebear.com/9.x/personas/svg?seed=Robert%20Zemeckis`,
};

const MAX_IMPORT_CHARS = 2_000_000;

export function openCsv({ run, previewImport, clearDatabase, setSchema, setQuery, onImported, currentSchema }) {
  closeCsv();
  const root = document.createElement('div');
  root.id = 'csv-modal';
  root.innerHTML = `
    <div class="csv-dialog" role="dialog" aria-label="Import">
      <header>
        <strong>Import</strong>
        <span class="csv-hint">CSV or JSON. Drop a column on a type to add a field, or anywhere else to create a type. Drop a file or a URL on the table.</span>
        <button id="csv-close" type="button">close</button>
      </header>
      <div class="csv-body">
        <aside>
          <button type="button" id="csv-file-btn">Open a file</button>
          <input id="csv-file" type="file" accept=".csv,.json,text/csv,application/json,text/plain">
          <button type="button" id="csv-url-btn">Open a URL</button>
          ${Object.keys(PRESETS).map((name) => `<button type="button" class="csv-preset" data-preset="${name}">${name}</button>`).join('')}
        </aside>
        <div class="csv-main">
          <section class="csv-table-pane" id="csv-table-pane">
            <div id="csv-table-drop" hidden>Drop to open</div>
            <div id="csv-json-cols" hidden></div>
            <div id="csv-json" hidden></div>
            <div id="csv-table-wrap"><p class="csv-empty">Open a file, or open a URL. You can also drop or paste one here.</p></div>
          </section>
          <section class="csv-schema-pane">
            <div id="csv-type-chips" hidden></div>
            <div class="csv-schema-frame">
              <div id="csv-schema"></div>
              <div id="csv-drop-hl" hidden></div>
              <div id="csv-drop-hint" hidden></div>
            </div>
          </section>
        </div>
      </div>
      <footer>
        <div class="csv-footer-note">
          <span id="csv-status"></span>
          <span id="csv-disclaimer">merge will append to the existing data. reset &amp; import will clear all data.</span>
        </div>
        <div class="csv-actions">
          <button id="csv-merge" type="button" disabled>Merge</button>
          <button id="csv-import" type="button" disabled>Reset &amp; Import</button>
        </div>
      </footer>
    </div>
    <div id="csv-url-modal" hidden>
      <div class="csv-url-dialog" role="dialog" aria-label="Open a URL">
        <strong>Open a URL</strong>
        <p>Address of a CSV or JSON file.</p>
        <input id="csv-url" type="url" placeholder="https://" spellcheck="false">
        <p id="csv-url-error"></p>
        <footer>
          <button type="button" id="csv-url-cancel">Cancel</button>
          <button type="button" id="csv-url-go" class="primary">Open</button>
        </footer>
      </div>
    </div>`;
  document.body.appendChild(root);

  const schemaHost = root.querySelector('#csv-schema');
  const schemaFrame = schemaHost.parentElement;
  const schemaEditor = window.monaco.editor.create(schemaHost, {
    automaticLayout: true,
    minimap: { enabled: false },
    fontSize: 13,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    lineHeight: 20,
    scrollBeyondLastLine: false,
    wordWrap: 'off',
    tabSize: 2,
    padding: { top: 8, bottom: 8 },
    glyphMargin: false,
    folding: false,
    lineNumbersMinChars: 3,
    renderLineHighlight: 'none',
    overviewRulerLanes: 2,
    hideCursorInOverviewRuler: true,
    scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
    theme: 'vs',
    language: 'zega-schema',
    value: '',
  });
  const schemaEl = {
    get value() { return schemaEditor.getValue(); },
    set value(text) { schemaEditor.setValue(text); },
  };
  const tableWrap = root.querySelector('#csv-table-wrap');
  const status = root.querySelector('#csv-status');
  const importBtn = root.querySelector('#csv-import');
  const mergeBtn = root.querySelector('#csv-merge');
  let headers = [];
  let rows = [];
  let rawText = '';
  let sourceKind = 'csv';
  let jsonValue = null;
  let jsonEditor = null;

  const refresh = () => {
    const ready = !/type\s+[A-Za-z_]/.test(schemaEl.value) || !rows.length;
    importBtn.disabled = ready;
    mergeBtn.disabled = ready;
    paintTable();
    paintChips();
  };

  const load = (text, label) => {
    typeOrigin.clear();
    let parsed;
    try { parsed = previewImport(text); } catch (error) { parsed = { error: String(error) }; }
    if (parsed.error) {
      headers = [];
      rows = [];
      sourceKind = 'csv';
      jsonValue = null;
      status.textContent = parsed.error;
      refresh();
      return;
    }
    headers = parsed.headers;
    rows = parsed.rows;
    rawText = text;
    sourceKind = parsed.kind || 'csv';
    jsonValue = parsed.value ?? null;
    status.textContent = `${label}: ${rows.length} rows`;
    refresh();
  };

  const urlModal = root.querySelector('#csv-url-modal');
  const urlInput = root.querySelector('#csv-url');
  const urlError = root.querySelector('#csv-url-error');
  const tablePane = root.querySelector('#csv-table-pane');
  const tableDrop = root.querySelector('#csv-table-drop');

  const fetchUrl = async (url) => {
    const problem = remoteAddressError(url);
    if (problem) {
      status.textContent = problem;
      urlError.textContent = problem;
      return;
    }
    status.textContent = `Loading ${url}…`;
    urlError.textContent = '';
    try {
      const response = await fetch(url, { redirect: 'error' });
      const landed = remoteAddressError(response.url);
      if (landed) throw new Error(landed);
      if (!response.ok) throw new Error(String(response.status));
      const text = await response.text();
      if (text.length > MAX_IMPORT_CHARS) throw new Error('larger than 2MB');
      schemaEl.value = '';
      load(text, url);
      urlModal.hidden = true;
    } catch (error) {
      const detail = error?.message || '';
      const message = /^(That|Only|The|larger)/.test(detail)
        ? detail
        : `Cannot read ${url}. A page can only fetch a public http or https address the server allows it to read.`;
      status.textContent = message;
      urlError.textContent = message;
    }
  };

  const openUrlModal = () => {
    urlError.textContent = '';
    urlModal.hidden = false;
    urlInput.focus();
    urlInput.select();
  };

  root.querySelectorAll('.csv-preset').forEach((button) => {
    button.onclick = () => {
      root.querySelectorAll('.csv-preset').forEach((b) => b.classList.toggle('active', b === button));
      schemaEl.value = '';
      load(PRESETS[button.dataset.preset], button.dataset.preset);
    };
  });
  root.querySelector('#csv-url-btn').onclick = openUrlModal;
  root.querySelector('#csv-url-cancel').onclick = () => { urlModal.hidden = true; };
  root.querySelector('#csv-url-go').onclick = () => {
    const url = urlInput.value.trim();
    if (!url) {
      urlError.textContent = 'Paste an address.';
      return;
    }
    fetchUrl(url);
  };
  urlInput.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') root.querySelector('#csv-url-go').click();
  });
  urlModal.addEventListener('click', (event) => {
    if (event.target === urlModal) urlModal.hidden = true;
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
  const externalDrag = (event) => {
    if (dragHeader) return false;
    const types = [...(event.dataTransfer?.types || [])];
    return types.includes('Files') || types.includes('text/uri-list') || types.includes('text/plain');
  };
  const showTableDrop = (event) => {
    const types = [...(event.dataTransfer?.types || [])];
    tableDrop.textContent = types.includes('Files') && !types.includes('text/uri-list')
      ? 'Drop to open file'
      : 'Drop to open URL';
    tableDrop.hidden = false;
    tablePane.classList.add('dropping');
  };
  tablePane.addEventListener('dragover', (event) => {
    if (!externalDrag(event)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'copy';
    showTableDrop(event);
  });
  tablePane.addEventListener('dragleave', (event) => {
    if (tablePane.contains(event.relatedTarget)) return;
    tableDrop.hidden = true;
    tablePane.classList.remove('dropping');
  });
  tablePane.addEventListener('drop', (event) => {
    if (!externalDrag(event)) return;
    event.preventDefault();
    tableDrop.hidden = true;
    tablePane.classList.remove('dropping');
    const file = event.dataTransfer.files?.[0];
    if (file) {
      file.text().then((text) => {
        schemaEl.value = '';
        load(text, file.name);
      });
      return;
    }
    const uri = event.dataTransfer.getData('text/uri-list')
      .split('\n')
      .map((line) => line.trim())
      .find((line) => line && !line.startsWith('#'));
    const plain = event.dataTransfer.getData('text/plain').trim();
    const url = /^https?:\/\//i.test(uri || '') ? uri : (/^https?:\/\//i.test(plain) ? plain : '');
    if (url) fetchUrl(url);
  });
  tablePane.tabIndex = 0;
  tablePane.addEventListener('paste', (event) => {
    const data = event.clipboardData;
    if (!data) return;
    const file = data.files?.[0];
    if (file) {
      event.preventDefault();
      file.text().then((text) => {
        schemaEl.value = '';
        load(text, file.name || 'Clipboard');
      });
      return;
    }
    const text = data.getData('text/plain').trim();
    if (!text) return;
    event.preventDefault();
    if (/^https?:\/\//i.test(text) && !/[\n\r]/.test(text)) {
      fetchUrl(text);
      return;
    }
    schemaEl.value = '';
    load(text, 'Clipboard');
  }, true);
  tablePane.focus();
  root.querySelector('#csv-close').onclick = closeCsv;
  root.addEventListener('click', (event) => { if (event.target === root) closeCsv(); });
  root.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    if (!urlModal.hidden) {
      urlModal.hidden = true;
      return;
    }
    closeCsv();
  });
  schemaEditor.onDidChangeModelContent(refresh);

  const highlight = root.querySelector('#csv-drop-hl');
  const hint = root.querySelector('#csv-drop-hint');

  const showDrop = (header, block) => {
    if (!header) {
      highlight.hidden = true;
      hint.hidden = true;
      return;
    }
    const draggedType = header.startsWith('zega-type:') ? header.slice('zega-type:'.length) : '';
    if (draggedType) {
      if (block && block.name !== draggedType) {
        placeHighlight(block);
        hint.textContent = `Drop to connect ${block.name} → ${draggedType}`;
      } else {
        highlight.hidden = true;
        hint.textContent = `Drop to connect ${draggedType} to a node`;
      }
      hint.hidden = false;
      return;
    }
    if (block) {
      placeHighlight(block);
      const edge = existingTypeFor(header, schemaEl.value);
      hint.textContent = edge && edge !== block.name
        ? `Drop to add ${header} → ${edge} to node ${block.name}`
        : `Drop to add ${header} to node ${block.name}`;
    } else {
      highlight.hidden = true;
      const typeName = typeNameFrom(header);
      const exists = new RegExp(`\\btype\\s+${typeName}\\b`).test(schemaEl.value);
      hint.textContent = exists
        ? `Drop to add ${header} → ${typeName} on a node`
        : `Drop to create node ${typeName}`;
    }
    hint.hidden = false;
  };
  const hideDrop = () => {
    highlight.hidden = true;
    hint.hidden = true;
  };

  schemaFrame.addEventListener('dragover', (event) => {
    event.preventDefault();
    showDrop(draggingHeader(), typeAtPoint(schemaEditor, event));
  });
  schemaFrame.addEventListener('dragleave', (event) => {
    if (schemaFrame.contains(event.relatedTarget)) return;
    hideDrop();
  });
  schemaFrame.addEventListener('drop', (event) => {
    event.preventDefault();
    hideDrop();
    const header = event.dataTransfer.getData('text/plain') || dragHeader;
    if (!header) return;
    const block = typeAtPoint(schemaEditor, event);
    if (header.startsWith('zega-type:')) {
      const draggedType = header.slice('zega-type:'.length);
      if (block && block.name !== draggedType) addEdge(schemaEl, block, draggedType);
      else status.textContent = `Drop ${draggedType} on a node to connect them.`;
      refresh();
      return;
    }
    if (block) addField(schemaEl, block, header, columnValues(header));
    else if (!addType(schemaEl, header)) {
      status.textContent = `${typeNameFrom(header)} already exists. Drop ${header} on a node to add ${ident(header)}.`;
    }
    refresh();
  });
  root.addEventListener('dragend', hideDrop);

  const apply = async (merge) => {
    const built = buildImport(schemaEl.value, headers, sourceKind);
    if (built.error) {
      status.textContent = built.error;
      return;
    }
    if (merge) {
      setSchema(mergeSchema(currentSchema?.() || '', built.schema));
    } else {
      await clearDatabase();
      setSchema(built.schema);
    }
    for (const mutation of built.mutations) {
      const value = await run(mutation, { quiet: true, sources: { "./import": rawText } });
      if (value == null) {
        status.textContent = merge
          ? 'Merge stopped on a row that did not apply. The output pane has the reason.'
          : 'Import stopped on a row that did not apply. The output pane has the reason.';
        return;
      }
    }
    setQuery(built.query);
    await run(built.query);
    onImported?.();
    closeCsv();
  };
  importBtn.onclick = () => apply(false);
  mergeBtn.onclick = () => apply(true);

  function placeHighlight(block) {
    const lineHeight = schemaEditor.getOption(window.monaco.editor.EditorOption.lineHeight);
    const layout = schemaEditor.getLayoutInfo();
    const top = schemaEditor.getTopForLineNumber(block.start + 1) - schemaEditor.getScrollTop();
    highlight.style.top = `${top}px`;
    highlight.style.height = `${(block.end - block.start + 1) * lineHeight}px`;
    highlight.style.left = `${layout.contentLeft}px`;
    const font = schemaEditor.getOption(window.monaco.editor.EditorOption.fontInfo);
    highlight.style.width = `${Math.ceil(blockWidth(schemaEditor.getValue(), block, `${font.fontSize}px ${font.fontFamily}`)) + 8}px`;
    highlight.hidden = false;
  }

  function paintChips() {
    const chips = root.querySelector('#csv-type-chips');
    const names = typeBlocks(schemaEl.value).map((block) => block.name);
    if (!names.length) {
      chips.hidden = true;
      chips.innerHTML = '';
      return;
    }
    chips.hidden = false;
    chips.innerHTML = `<span>Connect</span>${names.map((name) => `<button type="button" draggable="true" data-type="${escapeAttr(name)}">${escapeHtml(name)}</button>`).join('')}`;
    chips.querySelectorAll('button').forEach((button) => {
      button.addEventListener('dragstart', (event) => {
        dragHeader = `zega-type:${button.dataset.type}`;
        event.dataTransfer.setData('text/plain', dragHeader);
        event.dataTransfer.effectAllowed = 'copy';
      });
      button.addEventListener('dragend', () => {
        dragHeader = '';
        hideDrop();
      });
    });
  }

  function columnValues(header) {
    const index = headers.indexOf(header);
    return rows.map((row) => row[index] ?? '');
  }
  function bindHeaderDrag(scope) {
    scope.querySelectorAll('th').forEach((th) => {
      th.addEventListener('dragstart', (event) => {
        dragHeader = th.dataset.header;
        event.dataTransfer.setData('text/plain', th.dataset.header);
        event.dataTransfer.effectAllowed = 'copy';
        th.classList.add('dragging');
      });
      th.addEventListener('dragend', () => {
        dragHeader = '';
        th.classList.remove('dragging');
        hideDrop();
      });
    });
  }

  function showPrettyJson(value) {
    const host = root.querySelector('#csv-json');
    const cols = root.querySelector('#csv-json-cols');
    tableWrap.hidden = true;
    host.hidden = false;
    cols.hidden = false;
    const schema = schemaEl.value;
    cols.innerHTML = `<table><thead><tr>${headers.map((header) => {
      const on = covered(header, schema);
      return `<th draggable="true" data-header="${escapeAttr(header)}" class="${on ? 'covered' : ''}"><span class="csv-mark">${on ? '✓' : ''}</span>${escapeHtml(header)}</th>`;
    }).join('')}</tr></thead></table>`;
    bindHeaderDrag(cols);
    const text = JSON.stringify(value, null, 2);
    const monaco = window.monaco;
    if (!monaco) {
      host.textContent = text;
      return;
    }
    if (!jsonEditor) {
      jsonEditor = monaco.editor.create(host, {
        value: text,
        language: 'json',
        readOnly: true,
        domReadOnly: true,
        folding: true,
        foldingStrategy: 'indentation',
        showFoldingControls: 'always',
        automaticLayout: true,
        minimap: { enabled: false },
        fontSize: 13,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
        lineHeight: 20,
        scrollBeyondLastLine: false,
        wordWrap: 'on',
        tabSize: 2,
        lineNumbersMinChars: 3,
        renderLineHighlight: 'none',
        scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
        theme: 'vs',
      });
    } else {
      jsonEditor.setValue(text);
    }
    jsonEditor.layout();
  }

  function paintTable() {
    const cols = root.querySelector('#csv-json-cols');
    const host = root.querySelector('#csv-json');
    if (!headers.length) {
      cols.hidden = true;
      host.hidden = true;
      tableWrap.hidden = false;
      tableWrap.innerHTML = '<p class="csv-empty">Open a file, or open a URL. You can also drop or paste one here.</p>';
      return;
    }
    if (sourceKind === 'json' && jsonValue != null) {
      showPrettyJson(jsonValue);
      return;
    }
    cols.hidden = true;
    host.hidden = true;
    tableWrap.hidden = false;
    const schema = schemaEl.value;
    const head = headers.map((header) => {
      const on = covered(header, schema);
      return `<th draggable="true" data-header="${escapeAttr(header)}" class="${on ? 'covered' : ''}"><span class="csv-mark">${on ? '✓' : ''}</span>${escapeHtml(header)}</th>`;
    }).join('');
    const body = rows.slice(0, 40).map((row) => `<tr>${headers.map((_, i) => `<td>${escapeHtml(row[i] ?? '')}</td>`).join('')}</tr>`).join('');
    const more = rows.length > 40 ? `<p class="csv-empty">Showing 40 of ${rows.length} rows.</p>` : '';
    tableWrap.innerHTML = `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>${more}`;
    bindHeaderDrag(tableWrap);
  }
}

function mergeSchema(current, incoming) {
  const have = new Set(typeBlocks(current).map((block) => block.name));
  const adding = typeBlocks(incoming)
    .filter((block) => !have.has(block.name))
    .map((block) => incoming.split('\n').slice(block.start, block.end + 1).join('\n'));
  if (!adding.length) return current;
  const base = current.trim();
  return base ? `${base}\n\n${adding.join('\n\n')}\n` : `${incoming.trim()}\n`;
}

function closeCsv() {
  document.getElementById('csv-modal')?.remove();
}

let dragHeader = '';
function draggingHeader() {
  return dragHeader;
}

const typeOrigin = new Map();

function covered(header, schema) {
  const id = ident(header);
  if (new RegExp(`\\b${id}\\b`).test(schema)) return true;
  const typeName = typeNameFrom(header);
  return typeOrigin.get(typeName) === header && new RegExp(`\\btype\\s+${typeName}\\b`).test(schema);
}

function ident(header) {
  const cleaned = String(header).replace(/[^A-Za-z0-9]+/g, '');
  if (!cleaned) return 'col';
  if (/^[0-9]/.test(cleaned)) return `_${cleaned}`;
  return cleaned;
}

function typeNameFrom(header) {
  return ident(header);
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
  if (ident(header).toLowerCase() === 'name') return null;
  const typeName = typeNameFrom(header);
  return new RegExp(`\\btype\\s+${typeName}\\b`).test(schema) ? typeName : null;
}

function typeBlocks(src) {
  const lines = src.split('\n');
  const blocks = [];
  let current = null;
  let depth = 0;
  lines.forEach((text, index) => {
    if (!current) {
      const open = text.match(/^\s*type\s+([A-Za-z_][\w]*)\s*\{/);
      if (!open) return;
      current = { name: open[1], start: index, end: index };
      depth = 0;
    }
    for (const ch of text) {
      if (ch === '{') depth += 1;
      else if (ch === '}') depth -= 1;
    }
    if (current && depth <= 0) {
      current.end = index;
      blocks.push(current);
      current = null;
    }
  });
  return blocks;
}

function textWidth(text, font) {
  if (!textWidth.ctx) textWidth.ctx = document.createElement('canvas').getContext('2d');
  textWidth.ctx.font = font;
  return textWidth.ctx.measureText(text).width;
}

function blockWidth(value, block, font) {
  const lines = value.split('\n').slice(block.start, block.end + 1);
  return Math.max(0, ...lines.map((line) => textWidth(line, font)));
}

function typeAtPoint(editor, event) {
  const hit = editor.getTargetAtClientPoint(event.clientX, event.clientY);
  if (!hit?.position) return null;
  const line = hit.position.lineNumber - 1;
  const block = typeBlocks(editor.getValue()).find((item) => line >= item.start && line <= item.end);
  if (!block) return null;
  const layout = editor.getLayoutInfo();
  const host = editor.getDomNode().getBoundingClientRect();
  const font = editor.getOption(window.monaco.editor.EditorOption.fontInfo);
  const x = event.clientX - host.left - layout.contentLeft + editor.getScrollLeft();
  if (x > blockWidth(editor.getValue(), block, `${font.fontSize}px ${font.fontFamily}`) + 8) return null;
  return block;
}

function addType(textarea, header) {
  const typeName = typeNameFrom(header);
  if (new RegExp(`\\btype\\s+${typeName}\\b`).test(textarea.value)) return false;
  typeOrigin.set(typeName, header);
  const block = `type ${typeName} {\n  name: String\n}\n`;
  textarea.value = textarea.value.trim() ? `${textarea.value.trim()}\n\n${block}` : block;
  return true;
}

function addEdge(textarea, block, targetType) {
  const base = targetType.charAt(0).toLowerCase() + targetType.slice(1);
  const lines = textarea.value.split('\n');
  const body = lines.slice(block.start, block.end + 1).join('\n');
  let name = base;
  let n = 2;
  while (new RegExp(`\\b${name}\\b`).test(body)) {
    name = `${base}${n}`;
    n += 1;
  }
  lines.splice(block.end, 0, `  ${name} -> ${targetType}`);
  textarea.value = lines.join('\n');
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

export function parseSchema(src) {
  const types = [];
  const re = /type\s+([A-Za-z_][\w]*)\s*\{/g;
  let match = re.exec(src);
  while (match) {
    let depth = 1;
    let i = re.lastIndex;
    const start = i;
    while (i < src.length && depth > 0) {
      if (src[i] === '{') depth += 1;
      else if (src[i] === '}') depth -= 1;
      i += 1;
    }
    types.push({ name: match[1], fields: parseFields(src.slice(start, i - 1)) });
    re.lastIndex = i;
    match = re.exec(src);
  }
  return types;
}

function parseFields(body) {
  const fields = [];
  const lines = body.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].trim();
    if (!line || line.startsWith('//')) continue;
    const edge = line.match(/^([A-Za-z_][\w]*)\s*(?::\s*[A-Za-z_][\w]*\s*)?(->|<-)\s*([A-Za-z_][\w]*)(\[\])?/);
    if (edge) {
      fields.push({
        name: edge[1],
        edge: true,
        dir: edge[2],
        target: edge[3],
        many: Boolean(edge[4]),
        zql: edge[3],
      });
      if (line.includes('{')) i = skipBrace(lines, i);
      else if ((lines[i + 1] || '').trim().startsWith('{')) i = skipBrace(lines, i + 1);
      continue;
    }
    const prop = line.match(/^([A-Za-z_][\w]*)\??\s*:\s*([A-Za-z_][\w]*)/);
    if (prop) fields.push({ name: prop[1], edge: false, zql: prop[2] });
  }
  return fields;
}

function skipBrace(lines, start) {
  let depth = 0;
  let seen = false;
  for (let i = start; i < lines.length; i += 1) {
    for (const ch of lines[i]) {
      if (ch === '{') { depth += 1; seen = true; }
      else if (ch === '}') depth -= 1;
    }
    if (seen && depth <= 0) return i;
  }
  return start;
}

function columnsForType(type, headers) {
  return headers.filter((header) => ident(header) === type.name);
}

export function pointListsAtMembers(schema) {
  return schema.replace(
    /^(\s*[A-Za-z_][\w]*\??\s*)<-(\s*[A-Za-z_][\w]*\[\])/gm,
    '$1->$2',
  );
}

function fieldMatches(header, fieldName) {
  return ident(header).toLowerCase() === fieldName.toLowerCase();
}

function matchingFields(type, headers) {
  const pairs = [];
  for (const field of type.fields) {
    if (field.edge) continue;
    const header = headers.find((item) => fieldMatches(item, field.name));
    if (header) pairs.push({ field, header });
  }
  return pairs;
}

// The UI maps columns to a ZQL template. It never binds individual rows or
// converts their values; the engine receives the original text for every load.
export function buildImport(schema, headers, format) {
  schema = pointListsAtMembers(schema);
  const types = parseSchema(schema);
  if (!types.length) return { error: 'The schema has no types yet.' };
  const rowType = types.slice().sort((a, b) => matchingFields(b, headers).length - matchingFields(a, headers).length)[0];
  const fields = (type) => type.fields.filter((field) => !field.edge).map((field) => {
    let header = headers.find((item) => fieldMatches(item, field.name));
    if (field.name === 'name') {
      header = columnsForType(type, headers)[0] || (type === rowType ? header : undefined);
    }
    return header ? { field: field.name, header } : null;
  }).filter(Boolean);
  const visited = new Set();
  const selection = (type, ancestors = new Set()) => {
    const props = fields(type);
    if (!props.length || ancestors.has(type.name)) return null;
    visited.add(type.name);
    const path = new Set([...ancestors, type.name]);
    const edges = type.fields.filter((field) => field.edge).map((edge) => {
      const target = types.find((item) => item.name === edge.target);
      const child = target && selection(target, path);
      return child ? `${edge.name} ${edge.dir} ${child}` : null;
    }).filter(Boolean);
    return `${type.name}(${props.map(({ field, header }) => `${field}: $${JSON.stringify(header)}`).join(' && ')}) { ${props.map(({ field }) => field).join(' ')} ${edges.join(' ')} }`;
  };
  const mutations = [];
  // One graph per source row, exactly as native mutation csv/json behaves.
  for (const type of [rowType, ...types.filter((type) => type !== rowType)]) {
    if (visited.has(type.name)) continue;
    const root = selection(type);
    if (root) mutations.push(`mutation ${format} ["./import"] { ${root} }`);
  }
  if (!mutations.length) return { error: 'None of the columns match a field on a type yet.' };
  const query = `{ ${rowType.name} { ${fields(rowType).map(({ field }) => field).join(' ')} } }`;
  return { schema, mutations, query };
}

function remoteAddressError(raw) {
  let url;
  try { url = new URL(raw); } catch { return 'That address is not a URL.'; }
  if (url.username || url.password) return 'The address cannot include a password.';
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return 'Only http and https addresses are allowed.';
  if (blockedHost(url.hostname)) return 'That address points at a private network.';
  return '';
}

function blockedHost(host) {
  const name = host.replace(/^\[|\]$/g, '').toLowerCase();
  if (name === 'localhost' || name.endsWith('.localhost') || name.endsWith('.local') || name === 'metadata.google.internal') return true;
  if (name === '::1' || name === 'https://example.net/id/garnet') return true;
  if (name.startsWith('fe80:') || name.startsWith('fc') || name.startsWith('fd')) return true;
  const parts = name.split('.');
  if (parts.length !== 4 || parts.some((part) => !/^\d+$/.test(part))) return false;
  const n = parts.map(Number);
  if (n.some((part) => part > 255)) return true;
  if (n[0] === 0 || n[0] === 10 || n[0] === 127) return true;
  if (n[0] === 169 && n[1] === 254) return true;
  if (n[0] === 172 && n[1] >= 16 && n[1] <= 31) return true;
  if (n[0] === 192 && n[1] === 168) return true;
  return false;
}

function escapeHtml(value) {
  return String(value).replace(/[&<>]/g, (ch) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch]));
}

function escapeAttr(value) {
  return escapeHtml(value).replace(/"/g, '&quot;');
}
