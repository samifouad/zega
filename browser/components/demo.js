// The canvas runtime's working demo: the Monaco sample loaded into the wasm
// engine, the contract's example ZQL run against it, and its result bound to
// path-on-map through a view spec. Each "load" is a new notebook entry; the
// history list restores any entry from its snapshot, without re-querying.
import init, { ZegaWasm } from '../pkg/zega_wasm.js';
import { applyTheme } from '../theme.js';
import { createCanvas } from './canvas.js';
import { COMPONENTS } from './registry.js';
import { localStorageStore } from './storage.js';

const $ = (selector) => document.querySelector(selector);
const contract = COMPONENTS['path-on-map'].contract;
let theme = new URLSearchParams(location.search).get('theme') === 'dark' ? 'dark' : 'light';
applyTheme(theme);
$('#theme').textContent = theme === 'dark' ? 'light' : 'dark';

// The sample: schema, data and the example query, through the real engine.
await init();
const db = new ZegaWasm();
const sampleUrl = new URL(`../${contract.example.sample}`, import.meta.url);
const zql = await (await fetch(sampleUrl)).text();
const sources = {};
for (const location of JSON.parse(db.load_locations(zql, true))) {
  sources[location] = await (await fetch(new URL(location, new URL('../', import.meta.url)))).text();
}
db.apply_with_sources(zql, JSON.stringify(sources));
const query = (text) => JSON.parse(db.run(zql, text));
const result = query(contract.example.zql);
$('#zql').textContent = contract.example.zql;

const historyLimit = Number(new URLSearchParams(location.search).get('history')) || 100;
const canvas = createCanvas($('#stage'), { theme, historyLimit, store: localStorageStore('zega.components.demo') });
canvas.on(() => show());

function specFromControls() {
  const value = $('#value').value;
  return {
    component: 'path-on-map',
    version: '1',
    binding: { path: 'points[].at', ...(value ? { value } : {}), label: 'name' },
    options: { colorScale: $('#colorScale').value, view: $('#view').value, bearing: -30, showBuildings: $('#showBuildings').checked },
  };
}
function showHistory() {
  const list = $('#history');
  list.replaceChildren(...canvas.history.map((entry, i) => {
    const item = document.createElement('li');
    if (i === canvas.current) item.className = 'current';
    const button = document.createElement('button');
    const o = entry.spec.options;
    const edit = entry.meta.edit ? Object.entries(entry.meta.edit).map(([k, v]) => `${k.split('.').at(-1)} = ${JSON.stringify(v)}`).join(', ') : null;
    button.textContent = `#${entry.id} ${edit || `${entry.spec.binding.value ? entry.spec.binding.value.split('.').at(-1) : 'no value'} · ${o.colorScale} · ${o.view}`}${entry.parent ? ` ← #${entry.parent}` : ''}`;
    button.onclick = () => canvas.restore(i);
    item.append(button);
    return item;
  }));
}
function show() {
  const entry = canvas.history[canvas.current];
  $('#spec').textContent = entry ? JSON.stringify(entry.spec, null, 2) : '';
  showHistory();
}
async function load(spec) {
  $('#errors').hidden = true;
  try {
    await canvas.load(spec, result, { zql: contract.example.zql });
  } catch (error) {
    $('#errors').textContent = error.message;
    $('#errors').hidden = false;
  }
}
$('#load').onclick = () => load(specFromControls());
$('#undo').onclick = () => canvas.undo();
$('#clear').onclick = async () => { await canvas.clearHistory(); await load(contract.example.spec); };
$('#theme').onclick = async () => {
  theme = theme === 'dark' ? 'light' : 'dark';
  applyTheme(theme);
  $('#theme').textContent = theme === 'dark' ? 'light' : 'dark';
  await canvas.setTheme(theme);
};

// The notebook carries over reloads: reopen on its last entry, or start with the example.
await canvas.ready;
if (canvas.history.length) await canvas.restore(canvas.history.length - 1);
else await load(contract.example.spec);
// For tests and the console.
window.zegaComponents = { canvas, result, query, contract, load, specFromControls };
document.documentElement.dataset.ready = 'true';
