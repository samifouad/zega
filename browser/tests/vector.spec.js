import { test, expect } from './offline.js';
const cosine = (a,b) => a.reduce((s,x,i)=>s+x*b[i],0)/Math.sqrt(a.reduce((s,x)=>s+x*x,0)*b.reduce((s,x)=>s+x*x,0));
async function ready(page) {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if(await page.locator('#btn-play').textContent()==='pause')await page.locator('#btn-play').click();
  await page.locator('#btn-tickets').click();
  await expect(page.locator('.vector-count')).toHaveText('200 points');
}
for(const kind of ['vector2d','vector3d'])test(`${kind} renders the sample, ranks full vectors, and flags actual relationships`,async({page})=>{
  await ready(page);
  await page.locator(`[data-view=${kind}]`).click();
  const canvas=page.locator('.vector-stage canvas');
  await expect(canvas).toHaveAttribute('data-points','200');
  const raw=await page.evaluate(async()=>{
    const { ZegaWasm } = await import('/pkg/zega_wasm.js');
    const db=new ZegaWasm();db.import_base64(localStorage.getItem('zega.v2.since'));
    const graph=JSON.parse(db.graph());db.free();return graph;
  });
  const nodes=raw.nodes;
  expect(nodes).toHaveLength(200);
  const chosen=nodes.find(n=>n.id===20);
  const expected=nodes.filter(n=>n.id!==chosen.id).map(n=>({id:n.id,score:cosine(chosen.embedding,n.embedding)})).sort((a,b)=>b.score-a.score||a.id-b.id).slice(0,10);
  // Actual canvas hit-testing must open the same inspector and update neighbors.
  const point=await canvas.evaluate(c=>c.projectedPoints.find(p=>p.id===20));
  await canvas.click({position:{x:point.x,y:point.y}});
  await expect(page.locator('#node-inspector')).toContainText(chosen.title);
  const actual=await page.locator('.vector-nearest li').evaluateAll(rows=>rows.map(r=>({id:Number(r.dataset.node),score:Number(r.dataset.score)})));
  expect(actual.map(r=>r.id)).toEqual(expected.map(r=>r.id));
  actual.forEach((r,i)=>expect(r.score).toBeCloseTo(expected[i].score,12));
  await page.locator('#node-inspector button').click();
  await page.getByLabel('Links vs meaning',{exact:true}).check();
  const linked=new Set(raw.rels.map(r=>[Math.min(r.from,r.to),Math.max(r.from,r.to)].join(':')));
  const flags=[];
  const sorted=[...nodes].sort((a,b)=>a.id-b.id);
  for(let i=0;i<sorted.length;i++)for(let j=i+1;j<sorted.length;j++){
    const a=sorted[i],b=sorted[j],score=cosine(a.embedding,b.embedding),isLinked=linked.has(`${a.id}:${b.id}`);
    if(isLinked&&score<.8)flags.push({from:a.id,to:b.id,kind:'linked-but-far'});
    if(!isLinked&&score>=.8)flags.push({from:a.id,to:b.id,kind:'near-but-unlinked'});
  }
  expect(flags.some(f=>f.kind==='linked-but-far')).toBe(true);
  const engineFlags=await page.locator('.vector-view').evaluate(el=>el.vectorData.flags.map(({from,to,kind})=>({from,to,kind})));
  expect(engineFlags).toEqual(flags);
  await page.getByLabel('Colour by',{exact:true}).selectOption('status');
  await expect(page.locator('.vector-legend')).toContainText('Resolved');
  await page.getByLabel('Search glow',{exact:true}).fill('Password');
  const before=await canvas.evaluate(c=>c.projectedPoints);
  const box=await canvas.boundingBox();
  await page.mouse.move(box.x+box.width/2,box.y+box.height/2);
  await page.mouse.down();await page.mouse.move(box.x+box.width/2+40,box.y+box.height/2+20,{steps:5});await page.mouse.up();
  expect(await canvas.evaluate(c=>c.projectedPoints)).not.toEqual(before);
  await page.getByLabel('Search glow',{exact:true}).fill('');
  await page.getByLabel('Colour by',{exact:true}).selectOption('topic');
  await page.getByLabel('Links vs meaning',{exact:true}).uncheck();
  await page.screenshot({path:`../.tmp/${kind}.png`,fullPage:true});
  await page.reload();
  await expect(page.locator('.vector-count')).toHaveText('200 points');
  // Query-result scope, not the full database, drives projection.
  await page.evaluate(()=>window.monaco.editor.getEditors().find(e=>e.getDomNode()?.closest('#query')).setValue('{ Ticket(topic = "Billing") limit 10 { id title } }'));
  await expect(page.locator('.vector-count')).toHaveText('10 points');
});
