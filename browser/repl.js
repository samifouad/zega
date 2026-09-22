import init, { ZegaWasm } from './pkg/zega_wasm.js';
import { renderGraph } from './graph.js';

const LS_DB = 'zega.v2.since';
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
    &hops
    playsFor -> Team { name &hops }
  }
}`],
  ['Oilers arrivals', `{
  Team(name: "Oilers") {
    name
    playsFor <- Player { name &since }
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
  ['Hops from Canada', `{
  Country(name: "Canada") {
    born <- Player {
      name
      &hops
      playsFor -> Team { name &hops }
    }
  }
}`],
  ['Swedish players', `{
  Country(name: "Sweden") {
    name
    born <- Player {
      name
      salary
      playsFor -> Team { name }
    }
  }
}`],
];

function teamSeed(name, city, abbr, players) {
  const roster = players.map(([player, position, id, salary, since]) => {
    const joined = since == null ? '' : ` &since: ${since}`;
    return `playsFor <- Player(name: "${player}", position: "${position}", face: "${mug(id)}", salary: ${salary}) { name salary${joined} }`;
  }).join('\n    ');
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
    ['Connor McDavid', 'C', 8478402, 12500000, 2015],
    ['Leon Draisaitl', 'C', 8477934, 14000000, 2014],
    ['Evan Bouchard', 'D', 8479999, 10500000, 2018],
    ['Zach Hyman', 'LW', 8475786, 5500000, 2021],
    ['Ryan Nugent-Hopkins', 'C', 8476454, 5125000, 2011],
    ['Mattias Ekholm', 'D', 8475218, 4000000, 2023],
  ]),
  teamSeed('Maple Leafs', 'Toronto', 'TOR', [
    ['Auston Matthews', 'C', 8479318, 13250000, 2016],
    ['William Nylander', 'RW', 8477939, 11500000, 2016],
    ['Morgan Rielly', 'D', 8476853, 7500000, 2013],
    ['John Tavares', 'C', 8475166, 4389280, 2018],
  ]),
  teamSeed('Golden Knights', 'Vegas', 'VGK', [
    ['Jack Eichel', 'C', 8478403, 13500000, 2021],
    ['Mitch Marner', 'RW', 8478483, 12000000, 2025],
    ['Mark Stone', 'RW', 8475913, 9500000, 2017],
  ]),
  teamSeed('Avalanche', 'Colorado', 'COL', [
    ['Nathan MacKinnon', 'C', 8477492, 12604000, 2013],
    ['Martin Necas', 'C', 8480039, 11500000, 2025],
    ['Cale Makar', 'D', 8480069, 9000000, 2019],
    ['Brock Nelson', 'C', 8475754, 7500000, 2025],
    ['Devon Toews', 'D', 8478038, 7250000, 2020],
    ['Gabriel Landeskog', 'LW', 8476455, 7000000, 2011],
    ['Nazem Kadri', 'C', 8475172, 5600000, 2022],
  ]),
  teamSeed('Penguins', 'Pittsburgh', 'PIT', [
    ['Erik Karlsson', 'D', 8474578, 11500000, 2023],
    ['Sidney Crosby', 'C', 8471675, 8700000, 2005],
    ['Kris Letang', 'D', 8471724, 6100000, 2006],
    ['Evgeni Malkin', 'C', 8471215, 5500000, 2006],
    ['Bryan Rust', 'RW', 8475810, 5125000, 2014],
  ]),
  teamSeed('Capitals', 'Washington', 'WSH', [
    ['Alex Tuch', 'RW', 8477949, 10500000, 2026],
    ['Pierre-Luc Dubois', 'C', 8479400, 8500000, 2024],
    ['Jordan Kyrou', 'RW', 8479385, 8125000],
    ['Tom Wilson', 'RW', 8476880, 6500000, 2013],
    ['Alex Ovechkin', 'LW', 8471214, 4250000, 2005],
  ]),
  teamSeed('Lightning', 'Tampa Bay', 'TBL', [
    ['Andrei Vasilevskiy', 'G', 8476883, 9500000, 2014],
    ['Nikita Kucherov', 'RW', 8476453, 9500000, 2013],
    ['Brayden Point', 'C', 8478010, 9500000, 2016],
    ['Jake Guentzel', 'LW', 8477404, 9000000, 2024],
    ['Victor Hedman', 'D', 8475167, 8000000, 2009],
    ['Brandon Hagel', 'LW', 8479542, 6500000, 2022],
    ['Anthony Cirelli', 'C', 8478519, 6250000, 2017],
  ]),
  ...countrySeed('Canada', 'ca', [
    'Connor McDavid', 'Evan Bouchard', 'Zach Hyman', 'Ryan Nugent-Hopkins',
    'Morgan Rielly', 'John Tavares', 'Mitch Marner', 'Mark Stone',
    'Nathan MacKinnon', 'Cale Makar', 'Devon Toews', 'Nazem Kadri',
    'Sidney Crosby', 'Kris Letang', 'Bryan Rust', 'Pierre-Luc Dubois',
    'Jordan Kyrou', 'Tom Wilson', 'Brayden Point', 'Brandon Hagel', 'Anthony Cirelli',
  ]),
  ...countrySeed('United States', 'us', [
    'Auston Matthews', 'Jack Eichel', 'Brock Nelson', 'Alex Tuch', 'Jake Guentzel',
  ]),
  ...countrySeed('Sweden', 'se', [
    'Mattias Ekholm', 'William Nylander', 'Gabriel Landeskog', 'Erik Karlsson', 'Victor Hedman',
  ]),
  ...countrySeed('Germany', 'de', ['Leon Draisaitl']),
  ...countrySeed('Czechia', 'cz', ['Martin Necas']),
  ...countrySeed('Russia', 'ru', [
    'Alex Ovechkin', 'Evgeni Malkin', 'Nikita Kucherov', 'Andrei Vasilevskiy',
  ]),
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
    const elapsedUs = (performance.now() - started) * 1000;
    queryTime.textContent = elapsedUs < 1000
      ? `${Math.round(elapsedUs)} µs`
      : `${(elapsedUs / 1000).toFixed(2)} ms`;
    const value = JSON.parse(raw);
    try { localStorage.setItem(LS_DB, db.export_base64()); } catch (e) { console.error(e); }
    show(value, null);
    return value;
  } catch (e) {
    const failedUs = (performance.now() - started) * 1000;
    queryTime.textContent = failedUs < 1000
      ? `${Math.round(failedUs)} µs`
      : `${(failedUs / 1000).toFixed(2)} ms`;
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

const panesEl = document.getElementById('panes');
let splitX = Number(localStorage.getItem('zega.v2.split-x'));
let splitY = Number(localStorage.getItem('zega.v2.split-y'));
if (!Number.isFinite(splitX) || splitX <= 0 || splitX >= 1) splitX = 0.5;
if (!Number.isFinite(splitY) || splitY <= 0 || splitY >= 1) splitY = 1.15 / 2;

function applySplits() {
  panesEl.style.setProperty('--split-x', `${splitX}fr`);
  panesEl.style.setProperty('--split-x-rest', `${1 - splitX}fr`);
  panesEl.style.setProperty('--split-y', `${splitY}fr`);
  panesEl.style.setProperty('--split-y-rest', `${1 - splitY}fr`);
}
applySplits();

function bindSplit(id, axis) {
  const handle = document.getElementById(id);
  handle.addEventListener('pointerdown', (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add('dragging');
    const move = (ev) => {
      const rect = panesEl.getBoundingClientRect();
      if (axis === 'x') {
        splitX = Math.min(0.8, Math.max(0.2, (ev.clientX - rect.left) / rect.width));
        localStorage.setItem('zega.v2.split-x', String(splitX));
      } else {
        splitY = Math.min(0.8, Math.max(0.2, (ev.clientY - rect.top) / rect.height));
        localStorage.setItem('zega.v2.split-y', String(splitY));
      }
      applySplits();
    };
    const stop = () => {
      handle.classList.remove('dragging');
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', stop);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', stop);
  });
}
bindSplit('split-x', 'x');
bindSplit('split-y', 'y');

let opening = { nodes: [] };
try { opening = storedGraph(); } catch (e) { jsonEl.textContent = String(e); }

if (!opening.nodes.length) {
  reseed();
} else {
  showTour(0);
  startAutoplay();
}
