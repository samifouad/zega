import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { renderGraph } from './graph.js';

const LS_DB = 'zega.v2.playsfor';
const LS_SCHEMA = 'zega.v2.schema';
const LS_QUERY = 'zega.v2.query';

const mug = (id) => `https://assets.nhle.com/mugs/nhl/latest/${id}.png`;
const logo = (abbr) => `https://assets.nhle.com/logos/nhl/svg/${abbr}_light.svg`;
const flag = (code) => `https://flagcdn.com/w160/${code}.png`;

const SCHEMA = `type Team {
  name: String
  city: String
  logo: String

  playsFor <- Player[]
}

type Player {
  name: String
  position: String
  face: String
  salary: Int

  playsFor -> Team
  born -> Country
}

type Country {
  name: String
  flag: String

  born <- Player[]
}`;

const QUERY = `{
  Country(name: "Canada") {
    name
    born <- Player {
      name
      salary
      playsFor -> Team { name }
    }
  }
}`;

const TOUR = [
  ['Canadian players', QUERY],
  ['Russian born players', `{
  Country(name: "Russia") {
    name
    born <- Player {
      name
      salary
      playsFor -> Team { name }
    }
  }
}`],
  ['Players who make over $10M', `{
  Player(salary > 10000000) {
    name
    salary
    playsFor -> Team { name }
  }
}`],
  ['Oilers roster', `{
  Team(name: "Oilers") {
    name
    playsFor <- Player { name position salary }
  }
}`],
  ['Golden Knights', `{
  Team(name: "Golden Knights") {
    name
    playsFor <- Player { name salary born -> Country { name } }
  }
}`],
  ['Germany', `{
  Country(name: "Germany") {
    name
    born <- Player { name salary playsFor -> Team { name } }
  }
}`],
];

function teamSeed(name, city, abbr, players) {
  const roster = players.map(([player, position, id, salary]) =>
    `playsFor <- Player(name: "${player}", position: "${position}", face: "${mug(id)}", salary: ${salary}) { name salary }`
  ).join('\n    ');
  return `mutation {
  Team(name: "${name}", city: "${city}", logo: "${logo(abbr)}") {
    name
    ${roster}
  }
}`;
}

function countrySeed(name, code, players) {
  const links = players.map((player) =>
    `born <- link Player(name: "${player}") { name }`
  ).join('\n    ');
  return [
    `mutation { Country(name: "${name}", flag: "${flag(code)}") { name } }`,
    `mutation {
  Country(name: "${name}") {
    name
    ${links}
  }
}`,
  ];
}

const SEEDS = [
  teamSeed('Oilers', 'Edmonton', 'EDM', [
    ['Connor McDavid', 'C', 8478402, 12500000],
    ['Leon Draisaitl', 'C', 8477934, 14000000],
  ]),
  teamSeed('Maple Leafs', 'Toronto', 'TOR', [
    ['Auston Matthews', 'C', 8479318, 13250000],
  ]),
  teamSeed('Golden Knights', 'Vegas', 'VGK', [
    ['Mitch Marner', 'RW', 8478483, 12000000],
  ]),
  teamSeed('Avalanche', 'Colorado', 'COL', [
    ['Nathan MacKinnon', 'C', 8477492, 12604000],
    ['Cale Makar', 'D', 8480069, 9000000],
  ]),
  teamSeed('Penguins', 'Pittsburgh', 'PIT', [
    ['Sidney Crosby', 'C', 8471675, 8700000],
  ]),
  teamSeed('Capitals', 'Washington', 'WSH', [
    ['Alex Ovechkin', 'LW', 8471214, 4250000],
  ]),
  teamSeed('Lightning', 'Tampa Bay', 'TBL', [
    ['Nikita Kucherov', 'RW', 8476453, 9500000],
    ['Brayden Point', 'C', 8478010, 9500000],
  ]),
  ...countrySeed('Canada', 'ca', [
    'Connor McDavid', 'Mitch Marner', 'Nathan MacKinnon', 'Cale Makar', 'Sidney Crosby', 'Brayden Point',
  ]),
  ...countrySeed('United States', 'us', ['Auston Matthews']),
  ...countrySeed('Germany', 'de', ['Leon Draisaitl']),
  ...countrySeed('Russia', 'ru', ['Alex Ovechkin', 'Nikita Kucherov']),
];

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

let lastValue = null;

function namesIn(value, into = new Set()) {
  if (!value || typeof value !== 'object') return into;
  if (Array.isArray(value)) {
    value.forEach((item) => namesIn(item, into));
    return into;
  }
  if (typeof value.name === 'string') into.add(value.name);
  if (typeof value.title === 'string') into.add(value.title);
  for (const child of Object.values(value)) namesIn(child, into);
  return into;
}

const jsonSize = $('#json-size');

function show(value, error) {
  jsonEl.classList.toggle('error', Boolean(error));
  const text = error ? String(error) : JSON.stringify(value, null, 2);
  jsonEl.textContent = text;
  const kb = new TextEncoder().encode(text).length / 1024;
  jsonSize.textContent = kb < 10 ? `${kb.toFixed(2)} KB` : `${kb.toFixed(1)} KB`;
  if (error) return;
  lastValue = value;
  drawGraph();
}

const queryTime = $('#query-time');

function run(source) {
  localStorage.setItem(LS_SCHEMA, schemaEl.value);
  localStorage.setItem(LS_QUERY, queryEl.value);
  const started = performance.now();
  try {
    const raw = db.run(schemaEl.value, source);
    const elapsed = performance.now() - started;
    queryTime.textContent = elapsed < 10 ? `${elapsed.toFixed(2)} ms` : `${Math.round(elapsed)} ms`;
    const value = JSON.parse(raw);
    try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); }
    show(value, null);
    return value;
  } catch (e) {
    queryTime.textContent = `${(performance.now() - started).toFixed(2)} ms`;
    show(null, e);
    return null;
  }
}

$('#btn-run').onclick = () => run(queryEl.value);
$('#btn-seed').onclick = () => reseed();
$('#btn-clear').onclick = () => {
  localStorage.removeItem(LS_DB);
  localStorage.removeItem(LS_SCHEMA);
  localStorage.removeItem(LS_QUERY);
  location.reload();
};
queryEl.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
    e.preventDefault();
    clearTimeout(pending);
    run(queryEl.value);
  }
});

let pending = null;
function scheduleRun() {
  pauseAutoplay();
  localStorage.setItem(LS_SCHEMA, schemaEl.value);
  localStorage.setItem(LS_QUERY, queryEl.value);
  clearTimeout(pending);
  pending = setTimeout(() => {
    const source = queryEl.value.trim();
    if (!source || source.startsWith('mutation')) return;
    run(source);
  }, 350);
}
schemaEl.addEventListener('input', scheduleRun);
queryEl.addEventListener('input', scheduleRun);

let tourIndex = 0;
let tourTimer = null;
let playing = false;
const playBtn = $('#btn-play');
const tourBox = $('#tour-queries');

TOUR.forEach(([label], index) => {
  const button = document.createElement('button');
  button.textContent = label;
  button.onclick = () => {
    pauseAutoplay();
    showTour(index);
  };
  tourBox.appendChild(button);
});

function markTour() {
  [...tourBox.children].forEach((button, index) => {
    button.classList.toggle('active', index === tourIndex);
  });
}

function showTour(index) {
  tourIndex = index;
  queryEl.value = TOUR[index][1];
  markTour();
  run(TOUR[index][1]);
}

function pauseAutoplay() {
  playing = false;
  clearInterval(tourTimer);
  tourTimer = null;
  if (playBtn) playBtn.textContent = 'play';
}

function startAutoplay() {
  playing = true;
  playBtn.textContent = 'pause';
  clearInterval(tourTimer);
  tourTimer = setInterval(() => {
    showTour((tourIndex + 1) % TOUR.length);
  }, 5000);
}

playBtn.onclick = () => {
  if (playing) pauseAutoplay();
  else startAutoplay();
};

function reseed() {
  pauseAutoplay();
  schemaEl.value = SCHEMA;
  for (const seed of SEEDS) run(seed);
  showTour(0);
  startAutoplay();
}

function storedGraph() {
  return JSON.parse(db.graph());
}

function drawGraph() {
  let graph;
  try {
    graph = storedGraph();
  } catch (e) {
    graphEl.innerHTML = `<div class="empty">${e}</div>`;
    return;
  }
  renderGraph(graphEl, graph, namesIn(lastValue));
}

let opening = { nodes: [] };
try { opening = storedGraph(); } catch (e) { jsonEl.textContent = String(e); }

if (!opening.nodes.length) {
  reseed();
} else {
  showTour(0);
  startAutoplay();
}
