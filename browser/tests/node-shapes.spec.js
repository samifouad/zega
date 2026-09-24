import { test, expect } from './offline.js';
import { mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';

const pagePath = 'M -14 -22 H 9 L 18 -13 V 18 Q 18 22 14 22 H -14 Q -18 22 -18 18 V -18 Q -18 -22 -14 -22 Z';
const foldPath = 'M 9 -22 L 18 -13 H 9 Z';
const scan = '<svg xmlns="http://www.w3.org/2000/svg" width="180" height="220"><rect width="180" height="220" fill="#e6edf5"/><path d="M25 35H100 M25 65H155 M25 85H155 M25 105H125 M25 145H155 M25 165H155" stroke="#476887" stroke-width="7"/><circle cx="130" cy="190" r="13" fill="#ac542e"/></svg>';
const portrait = '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="#9dbab7"/><circle cx="50" cy="36" r="20" fill="#e8b58f"/><ellipse cx="50" cy="94" rx="37" ry="35" fill="#34566c"/></svg>';

async function ready(page) {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
}

async function fixture(page) {
  const requested = [];
  await page.route('https://images.example/**', async (route) => {
    requested.push(route.request().url());
    if (route.request().url().endsWith('missing.png')) return route.abort();
    await route.fulfill({ contentType: 'image/svg+xml', body: route.request().url().endsWith('photo.svg') ? portrait : scan });
  });
  await ready(page);
  await page.evaluate(async () => {
    const { renderGraph, stopSim } = await import('/graph.js');
    const container = document.querySelector('#graph');
    stopSim(container); container._graph = null;
    const types = [
      { name: 'Log', fields: [{kind:'prop',name:'name'},{kind:'prop',name:'status'},{kind:'prop',name:'at'},{kind:'prop',name:'tags'},{kind:'prop',name:'notes'},{kind:'prop',name:'reviewed'}] },
      { name: 'Contract', fields: [{kind:'prop',name:'name'},{kind:'prop',name:'scan'}] },
      { name: 'Player', fields: [{kind:'prop',name:'name'},{kind:'prop',name:'photo'}] },
      { name: 'Archive', fields: [{kind:'prop',name:'scan'}] },
      { name: 'Small', fields: [{kind:'prop',name:'name'}] },
      { name: 'Medium', fields: [{kind:'prop',name:'name'}] },
      { name: 'Large', fields: [{kind:'prop',name:'name'}] },
      { name: 'Remote', fields: [{kind:'prop',name:'name'},{kind:'prop',name:'scan'}] },
    ];
    const nodes = [
      {id:1,labels:['Log'],name:'Field notes',status:'Reviewed',at:{lat:51.05,lon:-114.06},tags:['research','2026'],notes:'The first five rows appear when zoomed in.',reviewed:true,x:0,y:0},
      {id:2,labels:['Contract'],name:'Signed agreement',scan:'https://images.example/scan.svg',x:-135,y:-40},
      {id:3,labels:['Player'],name:'Maya Chen',photo:'https://images.example/photo.svg',x:135,y:-40},
      {id:4,labels:['Archive'],scan:'https://images.example/missing.png',x:0,y:100},
      {id:5,labels:['Small'],name:'Size 1',x:-145,y:100},
      {id:6,labels:['Medium'],name:'Size 2',x:145,y:100},
      {id:7,labels:['Large'],name:'Size 3',x:0,y:-100},
      {id:8,labels:['Remote'],name:'Outside viewport',scan:'https://images.example/lazy.svg',x:10000,y:10000},
    ].map((n) => ({...n,fx:n.x,fy:n.y}));
    const graph = {nodes,rels:[{id:1,from:2,to:1,type:'describes'},{id:2,from:1,to:3,type:'author'},{id:3,from:1,to:4,type:'archives'}]};
    const view = {nodes:{Log:{shape:'document',size:2},Contract:{shape:'document',size:2,image:'scan'},Player:{shape:'circle',size:2,image:'photo'},Archive:{shape:'document',image:'scan'},Small:{size:1},Medium:{size:2},Large:{size:3},Remote:{shape:'document',image:'scan'}}};
    // Start with a known viewport, while using production pointer/zoom/render paths.
    container._graph = {capture:() => ({positions:new Map(),view:{scale:1.25,tx:400,ty:235}})};
    renderGraph(container, graph, new Set(), null, view, types);
    container._sim.stop();
  });
  await expect(page.locator('[data-node="2"]')).toHaveAttribute('data-image', 'loaded');
  return requested;
}

const node = (page, id) => page.locator(`#graph g[data-node="${id}"]`);
async function zoom(page) {
  for (let i=0;i<5;i++) await page.locator('[data-zoom="in"]').click();
}

test('document outline, fold, icon, clipped images and failure fallback', async ({ page }) => {
  const requests = await fixture(page);
  await expect(node(page,1).locator('.document-page')).toHaveAttribute('d', pagePath);
  await expect(node(page,1).locator('.document-fold')).toHaveAttribute('d', foldPath);
  await expect(node(page,1).locator('.document-icon path')).toHaveCount(6);
  for (const [id, clip] of [[2,'path'],[3,'circle']]) {
    const image = node(page,id).locator('.node-image');
    await expect(image).toHaveAttribute('visibility','visible');
    expect(await image.getAttribute('preserveAspectRatio')).toBe('xMidYMid slice');
    const clipId = (await image.getAttribute('clip-path')).slice(4,-1);
    await expect(page.locator(`${clipId} ${clip}`)).toHaveCount(1);
  }
  await expect(node(page,2).locator('.document-fold')).toHaveAttribute('d',foldPath);
  await expect(node(page,4)).toHaveAttribute('data-image','failed');
  await expect(node(page,4).locator('.node-image')).toHaveCount(0);
  await expect(node(page,4).locator('.document-icon')).toBeVisible();
  await expect(node(page,8)).toHaveAttribute('data-image','pending');
  expect(requests).not.toContain('https://images.example/lazy.svg');
  await expect(node(page,8).locator('.node-image')).not.toHaveAttribute('href', /./);
  for (const image of await page.locator('#graph .node-image').all()) {
    await expect(image).toHaveAttribute('referrerpolicy', 'no-referrer');
  }
  await expect(page.locator('meta[name="referrer"]')).toHaveAttribute('content', 'no-referrer');
});

test('plain cross-origin images load without CORS and send no referrer', async ({page}) => {
  const requests = [];
  const server = createServer((request, response) => {
    requests.push(request.headers);
    response.writeHead(200, {'Content-Type': 'image/svg+xml'});
    response.end(scan); // Deliberately no Access-Control-Allow-Origin.
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    await fixture(page);
    await page.evaluate(async (url) => {
      const {drawNode, nodeGeometry} = await import('/node-display.js');
      const g = document.querySelector('g[data-node="2"]'); g.replaceChildren();
      const visual = drawNode(g, document.querySelector('#graph defs'), nodeGeometry({shape:'document'}), url, '#aaa', {heading:'Plain image',rows:[]});
      visual.update(true, 1);
    }, `http://127.0.0.1:${server.address().port}/scan.svg`);
    await expect(node(page,2)).toHaveAttribute('data-image', 'loaded');
    expect(requests).toHaveLength(1);
    expect(requests[0].referer).toBeUndefined();
    expect(requests[0].origin).toBeUndefined();
    await expect(node(page,2).locator('image')).toHaveAttribute('referrerpolicy', 'no-referrer');
  } finally {
    await new Promise(resolve => server.close(resolve));
  }
});

async function overlaps(page) {
  return page.evaluate(() => {
    const rect = el => el.getBoundingClientRect();
    const captions = [...document.querySelectorAll('#graph .cap, #graph .node-plate')];
    const labels = [...document.querySelectorAll('#graph .rlab')].filter(el => rect(el).width > 0 && getComputedStyle(el).visibility !== 'hidden');
    const intersects = (a,b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
    return labels.flatMap(label => captions.filter(cap => intersects(rect(label),rect(cap))).map(cap => ({label:label.textContent,node:cap.closest('[data-node]').dataset.node,kind:cap.getAttribute('class')})));
  });
}

// Pairs of visible edge labels whose painted boxes intersect.
async function labelOverlaps(page) {
  return page.evaluate(() => {
    const labels = [...document.querySelectorAll('#graph .rlab')]
      .filter(el => el.getBoundingClientRect().width > 0 && getComputedStyle(el).visibility !== 'hidden')
      .map(el => ({ rel: el.dataset.rel, text: el.textContent, box: el.getBoundingClientRect() }));
    const intersects = (a,b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
    return labels.flatMap((a, i) => labels.slice(i + 1).filter(b => intersects(a.box, b.box)).map(b => `${a.text}#${a.rel} × ${b.text}#${b.rel}`));
  });
}

// zegadb/zega#42: a mesh of 30 nodes and 80 edges, fixed in place.
async function denseMesh(page) {
  await ready(page);
  await page.evaluate(async () => {
    const {renderGraph,stopSim} = await import('/graph.js');
    const container=document.querySelector('#graph'); stopSim(container);
    const nodes=Array.from({length:30},(_,i) => ({id:i+1,labels:['Person'],name:`Person ${i+1}`,x:(i%6)*120,y:Math.floor(i/6)*95})).map(n => ({...n,fx:n.x,fy:n.y}));
    const types=['follows','likes','knows','blocks'];
    const rels=[];
    const add=(from,to) => rels.push({id:rels.length+1,from,to,type:types[rels.length%types.length]});
    for (const n of nodes) {
      const col=(n.id-1)%6, row=Math.floor((n.id-1)/6);
      if (col<5) add(n.id,n.id+1);
      if (row<4) add(n.id,n.id+6);
      if (col<5 && row<4) add(n.id,n.id+7);
    }
    for (const [from,to] of [[1,9],[9,1],[4,16],[16,4],[7,20],[13,26],[22,11],[30,18],[25,14],[28,10],[3,3]]) add(from,to);
    container._graph={capture:() => ({positions:new Map(),view:{scale:1,tx:100,ty:50}})};
    renderGraph(container,{nodes,rels},new Set(),null,{nodes:{Person:{size:1}}},[{name:'Person',fields:[{kind:'prop',name:'name'}]}]);
    container._sim.stop();
  });
}

const hiddenLabels = (page) => page.locator('#graph .rlab').evaluateAll(els => els.filter(el => getComputedStyle(el).visibility === 'hidden').map(el => el.dataset.rel));

test('dense mesh: every edge label is shown and no two overlap', async ({page}) => {
  await denseMesh(page);
  const labels = page.locator('#graph .rlab');
  await expect(labels).toHaveCount(80);
  await expect.poll(() => labelOverlaps(page)).toEqual([]);
  await expect.poll(() => overlaps(page)).toEqual([]);
  // Hiding is the fallback, not the answer: this mesh has room for all 80.
  expect(await hiddenLabels(page)).toEqual([]);
  // The same graph gives the same layout on every frame.
  const positions = () => labels.evaluateAll(els => els.map(el => [el.getAttribute('x'), el.getAttribute('y'), getComputedStyle(el).visibility]));
  const first = await positions();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  expect(await positions()).toEqual(first);
});

test('a label with no free slot is hidden, and shown while its edge is pointed at', async ({page}) => {
  await ready(page);
  // Every pair of eight nodes on a small ring: 28 edges crossing one small
  // middle, with no room for 28 labels.
  await page.evaluate(async () => {
    const {renderGraph,stopSim} = await import('/graph.js');
    const container=document.querySelector('#graph'); stopSim(container);
    const nodes=Array.from({length:8},(_,i) => ({id:i+1,labels:['Person'],name:`P${i+1}`,x:Math.round(Math.cos(i*Math.PI/4)*80),y:Math.round(Math.sin(i*Math.PI/4)*80)})).map(n => ({...n,fx:n.x,fy:n.y}));
    const rels=[];
    for (let a=1;a<=8;a++) for (let b=a+1;b<=8;b++) rels.push({id:rels.length+1,from:a,to:b,type:['follows','likes','knows'][rels.length%3]});
    container._graph={capture:() => ({positions:new Map(),view:{scale:1,tx:400,ty:240}})};
    renderGraph(container,{nodes,rels},new Set(),null,{nodes:{Person:{size:1}}},[{name:'Person',fields:[{kind:'prop',name:'name'}]}]);
    container._sim.stop();
  });
  await expect(page.locator('#graph .rlab')).toHaveCount(28);
  await expect.poll(() => labelOverlaps(page)).toEqual([]);
  await expect.poll(() => overlaps(page)).toEqual([]);
  const hidden = await hiddenLabels(page);
  expect(hidden.length).toBeGreaterThan(0);
  expect(hidden.length).toBeLessThan(28);
  const rel = hidden[0];
  const label = page.locator(`#graph .rlab[data-rel="${rel}"]`);
  await page.locator(`#graph path[data-rel="${rel}"]`).dispatchEvent('pointerenter');
  await expect(label).toHaveCSS('visibility', 'visible');
  await page.locator(`#graph path[data-rel="${rel}"]`).dispatchEvent('pointerleave');
  await expect(label).toHaveCSS('visibility', 'hidden');
});

test('archives stays clear of Field notes and every caption and node body', async ({page}) => {
  await fixture(page);
  await expect(page.locator('.rlab[data-rel="3"]')).toHaveText('archives');
  await expect.poll(() => overlaps(page)).toEqual([]);
  await zoom(page);
  await expect.poll(() => overlaps(page)).toEqual([]);
});

test('dense fixed graph has no label/caption or label/body intersections', async ({page}) => {
  await ready(page);
  await page.evaluate(async () => {
    const {renderGraph,stopSim} = await import('/graph.js');
    const container=document.querySelector('#graph'); stopSim(container);
    const nodes=Array.from({length:20},(_,i) => ({id:i+1,labels:[i%2?'Page':'Person'],name:`Field notes ${i+1}`,x:(i%5)*125,y:Math.floor(i/5)*115})).map(n => ({...n,fx:n.x,fy:n.y}));
    const rels=[];
    for(const n of nodes) {
      if(n.id%5) rels.push({id:rels.length+1,from:n.id,to:n.id+1,type:'describes'});
      if(n.id<=15) rels.push({id:rels.length+1,from:n.id,to:n.id+5,type:'archives'});
    }
    // Include parallel curves and a self loop, with repeatable screen positions.
    rels.push({id:32,from:1,to:2,type:'reviews'},{id:33,from:2,to:1,type:'cites'},{id:34,from:3,to:3,type:'revises'});
    container._graph={capture:() => ({positions:new Map(),view:{scale:1,tx:100,ty:70}})};
    renderGraph(container,{nodes,rels},new Set(),null,{nodes:{Page:{shape:'document',size:2},Person:{size:1}}},[{name:'Page',fields:[{kind:'prop',name:'name'}]},{name:'Person',fields:[{kind:'prop',name:'name'}]}]);
    container._sim.stop();
  });
  await expect(page.locator('#graph .rlab:visible')).toHaveCount(34);
  await expect.poll(() => overlaps(page)).toEqual([]);
  const positions = () => page.locator('#graph .rlab').evaluateAll(els => els.map(el => [el.getAttribute('x'),el.getAttribute('y')]));
  const before = await positions();
  // Repeated production animation frames must give the same placement.
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  expect(await positions()).toEqual(before);
});

test('paper is white in light mode and grey with AA dark ink in dark mode', async ({page}) => {
  await fixture(page);
  await zoom(page);
  for (const [theme,paper] of [['light','rgb(255, 255, 255)'],['dark','rgb(203, 213, 225)']]) {
    await page.evaluate(async theme => (await import('/theme.js')).applyTheme(theme),theme);
    for (const plate of await page.locator('.document-page').all()) {
      expect(await plate.evaluate(el => getComputedStyle(el).fill)).toBe(paper);
    }
    const contrast = await node(page,1).evaluate((g,theme) => {
      const luminance = color => {
        const rgb = color.match(/[\d.]+/g).slice(0,3).map(Number).map(v => v/255).map(v => v <= .04045 ? v/12.92 : ((v+.055)/1.055)**2.4);
        return rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722;
      };
      const background = luminance(getComputedStyle(g.querySelector('.document-mini rect')).fill);
      return [...g.querySelectorAll(theme === 'dark' ? '.document-mini text, .document-icon path' : '.document-mini text')].map(el => {
        const style = getComputedStyle(el);
        const ink = luminance(el.tagName === 'text' ? style.fill : style.stroke);
        return (Math.max(ink,background)+.05)/(Math.min(ink,background)+.05);
      });
    }, theme);
    // Light-mode icon ink retains its existing muted treatment.
    for (const ratio of contrast) expect(ratio).toBeGreaterThanOrEqual(4.5);
    if (theme === 'dark') expect(await node(page,1).locator('.document-fold').evaluate(el => getComputedStyle(el).fill)).toBe('rgb(148, 163, 184)');
  }
});

test('size scales the geometry and curved edges terminate on both outlines', async ({ page }) => {
  await fixture(page);
  const widths = [];
  for (const id of [5,6,7]) widths.push((await node(page,id).locator('.node-plate').boundingBox()).width);
  expect(widths[1]/widths[0]).toBeCloseTo(1.5,4);
  expect(widths[2]/widths[0]).toBeCloseTo(2,4);
  const endpoints = await page.evaluate(() => {
    const svg = document.querySelector('#graph svg');
    const edge = document.querySelector('#graph .viewport > path[marker-end]');
    const nums = edge.getAttribute('d').match(/-?[\d.]+(?:e[-+]?\d+)?/g).map(Number);
    return [[2,nums[0],nums[1]],[1,nums[4],nums[5]]].map(([id,x,y]) => {
      const plate = document.querySelector(`g[data-node="${id}"] .node-plate`);
      const viewport = document.querySelector('#graph .viewport');
      const point = svg.createSVGPoint(); point.x=x;point.y=y;
      const local = point.matrixTransform(plate.getCTM().inverse().multiply(viewport.getCTM()));
      // A tiny step toward the centre is in the fill, away is outside.
      const inner = svg.createSVGPoint(), outer = svg.createSVGPoint();
      inner.x=local.x*.999;inner.y=local.y*.999;outer.x=local.x*1.001;outer.y=local.y*1.001;
      return [plate.isPointInFill(inner),plate.isPointInFill(outer)];
    });
  });
  expect(endpoints).toEqual([[true,false],[true,false]]);
});

test('self relationships loop outside a scaled page', async ({page}) => {
  await fixture(page);
  await page.evaluate(async () => {
    const {renderGraph,stopSim} = await import('/graph.js');
    const container=document.querySelector('#graph');stopSim(container);container._graph=null;
    renderGraph(container,{nodes:[{id:1,labels:['Log'],name:'Self',fx:0,fy:0}],rels:[{id:1,from:1,to:1,type:'revises'}]},new Set(),null,{nodes:{Log:{shape:'document',size:3}}},[{name:'Log',fields:[{kind:'prop',name:'name'}]}]);
    container._sim.stop();
  });
  const edge=page.locator('#graph .viewport > path[marker-end]');
  await expect(edge).toHaveAttribute('d',/ C /);
  const points=(await edge.getAttribute('d')).match(/-?[\d.]+/g).map(Number);
  expect(points[0]).toBeLessThan(0);expect(points[6]).toBeGreaterThan(0);
  expect(points[3]).toBeLessThan(-100);expect(points[5]).toBeLessThan(-100);
});

test('Space previews each shape with readable fields, focus trapping and all closing gestures', async ({ page }) => {
  await fixture(page);
  await node(page,1).locator('.node-plate').click();
  await expect(node(page,1)).toHaveAttribute('aria-pressed','true');
  await page.keyboard.press('Space');
  const dialog = page.getByRole('dialog', {name:'Field notes'});
  await expect(dialog).toBeVisible();
  await expect(dialog.locator('thead')).toHaveText('FieldValue');
  await expect(dialog.locator('tbody tr')).toHaveText(['nameField notes','statusReviewed','atlat: 51.05; lon: -114.06','tagsresearch; 2026','notesThe first five rows appear when zoomed in.','reviewedtrue']);
  await expect(dialog.getByRole('button')).toBeFocused();
  for (const key of ['Tab','Shift+Tab']) { await page.keyboard.press(key); await expect(dialog.getByRole('button')).toBeFocused(); }
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0); await expect(node(page,1)).toBeFocused();
  await page.keyboard.press('Space'); await expect(dialog).toBeVisible();
  await page.keyboard.press('Space'); await expect(dialog).toHaveCount(0);
  for (const [id,title] of [[2,'Signed agreement'],[3,'Maya Chen'],[4,'Archive #4']]) {
    await node(page,id).locator('.node-plate').click(); await page.keyboard.press('Space');
    await expect(page.getByRole('dialog',{name:title})).toBeVisible();
    await page.mouse.click(5,5); await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(node(page,id)).toBeFocused();
  }
});

test('zoom mini shares preview content, truncates to five rows and culls offscreen pages', async ({ page }) => {
  await fixture(page);
  await expect(page.locator('.document-mini')).toHaveCount(0);
  await zoom(page);
  await expect(node(page,1).locator('.document-mini')).toBeVisible();
  await expect(node(page,1).locator('.document-mini text')).toHaveCount(11);
  await expect(node(page,1).locator('.document-mini text').first()).toHaveText('Field notes');
  await expect(node(page,1).locator('.document-mini')).toContainText('Reviewed');
  await expect(node(page,1).locator('.document-mini')).not.toContainText('reviewed');
  await expect(node(page,8).locator('.document-mini')).toHaveCount(0);
  for (let i=0;i<5;i++) await page.locator('[data-zoom="out"]').click();
  await expect(page.locator('.document-mini')).toHaveCount(0);
});

test('lazy image completion and failure leave the page outline unchanged', async ({ page }) => {
  let release;
  const delayed = new Promise((resolve) => { release=resolve; });
  await fixture(page);
  await page.route('https://images.example/delayed.svg', async route => { await delayed; await route.fulfill({contentType:'image/svg+xml',body:scan}); });
  await page.evaluate(async () => {
    const {drawNode,nodeGeometry} = await import('/node-display.js');
    const g=document.querySelector('g[data-node="4"]');g.replaceChildren();
    const visual=drawNode(g,document.querySelector('#graph defs'),nodeGeometry({shape:'document',size:3}),'https://images.example/delayed.svg','#aaa',{heading:'Delayed',rows:[]});
    visual.update(true,1);
  });
  const before=await node(page,4).locator('.node-plate').boundingBox();
  release();
  await expect(node(page,4)).toHaveAttribute('data-image','loaded');
  expect(await node(page,4).locator('.node-plate').boundingBox()).toEqual(before);
});

test('checked WASM display reaches the graph and survives reload; URL errors underline the field', async ({page}) => {
  await ready(page);
  await page.locator('#btn-clear').click();
  const set = (pane,value) => page.evaluate(({pane,value}) => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${pane}`)).setValue(value),{pane,value});
  await set('query','');
  await set('schema','schema { type Note { name: String scan?: String<url> } display { graph { Note(@shape: document, @size: 3, @image: &scan) } } } mutation { Note(name: "Engine to explorer") }');
  await set('query','{ Note { @id name } }');
  await page.locator('#btn-run').click();
  await expect(page.locator('g[data-shape="document"]')).toHaveCount(1);
  await expect(page.locator('g[data-shape="document"]')).toHaveAttribute('data-size','3');
  await page.reload();
  await expect(page.locator('g[data-shape="document"]')).toHaveCount(1);
  // Nodes deliberately drift; click their current centre without waiting for animation to stop.
  const bounds = await page.locator('g[data-shape="document"] .node-plate').boundingBox();
  await page.mouse.click(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  await page.keyboard.press('Space');
  await expect(page.getByRole('dialog',{name:'Engine to explorer'})).toBeVisible(); await page.keyboard.press('Escape');
  await set('schema','type Note { scan: String } display { graph { Note(@image: &scan) } }');
  await expect.poll(() => page.evaluate(() => window.monaco.editor.getModelMarkers({owner:'zega'}).map(m => m.message).join('\n'))).toContain('declare `scan: String<url>` on Note');
});

test('evidence: light, dark, preview and zoom', async ({page}) => {
  await fixture(page); await mkdir('../evidence/node-shapes',{recursive:true});
  await page.mouse.move(10,10);
  await page.locator('#graph').screenshot({path:'../evidence/node-shapes/graph-light.png'});
  await page.evaluate(async () => (await import('/theme.js')).applyTheme('dark'));
  const paper = await node(page,1).locator('.document-page').evaluate(el => getComputedStyle(el).fill);
  expect(paper).toBe('rgb(203, 213, 225)');
  expect(await overlaps(page)).toEqual([]);
  await page.locator('#graph').screenshot({path:'../evidence/node-shapes/graph-dark.png'});
  await node(page,1).focus(); await page.keyboard.press('Space');
  await page.getByRole('dialog').screenshot({path:'../evidence/node-shapes/preview.png'});
  await page.keyboard.press('Escape');
  await zoom(page); await expect(node(page,1).locator('.document-mini')).toBeVisible();
  await page.mouse.move(10,10);
  await page.locator('#graph').screenshot({path:'../evidence/node-shapes/zoom.png'});
});
