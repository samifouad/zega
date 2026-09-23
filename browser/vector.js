// Native Canvas: no external renderer, network requests, or UI-side vector math.
const palette = ['#3d996d', '#9479d5', '#df9456', '#539dc7', '#d3668d', '#909652'];
const label = (n) => String(n.title ?? n.name ?? n.id);
export function renderVector(container, graph, kind, theme, analyze, onNode) {
  const root = document.createElement('div'); root.className = 'vector-view';
  const toolbar = document.createElement('div'); toolbar.className = 'vector-toolbar';
  const title = document.createElement('strong'); title.textContent = kind === 'vector2d' ? 'Meaning map' : 'Point cloud';
  const count = document.createElement('span'); count.className = 'vector-count';
  const colourLabel = document.createElement('label'); colourLabel.textContent = 'Colour by ';
  const colour = document.createElement('select'); colour.setAttribute('aria-label', 'Colour by');
  const fields = [...new Set(graph.nodes.flatMap(n => Object.keys(n).filter(k => !['id','labels'].includes(k) && typeof n[k] !== 'object')))];
  for (const field of fields) colour.add(new Option(field,field));
  if (fields.includes('topic')) colour.value = 'topic';
  colourLabel.append(colour);
  const search = document.createElement('input'); search.type = 'search'; search.placeholder = 'Search glow'; search.setAttribute('aria-label', 'Search glow');
  const linksLabel = document.createElement('label');
  const linksToggle = document.createElement('input'); linksToggle.type = 'checkbox'; linksToggle.setAttribute('aria-label','Links vs meaning');
  linksLabel.append(linksToggle, ' Links vs meaning');
  const reset = document.createElement('button'); reset.textContent = 'Reset view';
  toolbar.append(title,count,colourLabel,search,linksLabel,reset);
  const stage = document.createElement('div'); stage.className = 'vector-stage';
  const canvas = document.createElement('canvas'); canvas.setAttribute('aria-label', kind === 'vector2d' ? '2D meaning map' : '3D point cloud');
  canvas.tabIndex = 0;
  const side = document.createElement('aside'); side.className = 'vector-side';
  const nearestTitle = document.createElement('h3'); nearestTitle.textContent = 'Similar to this';
  const hint = document.createElement('p'); hint.textContent = 'Select a ticket to see its nearest neighbours.';
  const kLabel = document.createElement('label'); kLabel.textContent = 'Neighbours ';
  const kInput = document.createElement('input'); kInput.type = 'number'; kInput.min = '1'; kInput.max = '100'; kInput.value = '10'; kInput.setAttribute('aria-label','Neighbours'); kLabel.append(kInput);
  const nearest = document.createElement('ol'); nearest.className = 'vector-nearest';
  const flagsTitle = document.createElement('h3'); flagsTitle.textContent = 'Links vs meaning';
  const thresholdLabel = document.createElement('label'); thresholdLabel.textContent = 'Near score ≥ ';
  const threshold = document.createElement('input'); threshold.type = 'number'; threshold.step = '0.05'; threshold.value = '0.8'; threshold.setAttribute('aria-label','Near score threshold'); thresholdLabel.append(threshold);
  const summary = document.createElement('p'); summary.className = 'vector-flag-summary';
  const flags = document.createElement('ul'); flags.className = 'vector-flags';
  const picker = document.createElement('select'); picker.setAttribute('aria-label','Select vector node'); picker.add(new Option('Select a ticket…',''));
  const legend = document.createElement('div'); legend.className = 'vector-legend';
  side.append(nearestTitle,hint,picker,kLabel,nearest,flagsTitle,thresholdLabel,summary,flags);
  stage.append(canvas,side);
  const note = document.createElement('p'); note.className = 'vector-note'; note.textContent = 'PCA positions are approximate. Similarity scores come from full vectors. Drag to '+(kind === 'vector3d' ? 'rotate; Shift-drag to pan.' : 'pan.')+' Scroll to zoom.';
  root.append(toolbar,legend,stage,note); container.replaceChildren(root);
  const ctx = canvas.getContext('2d');
  const byId = new Map(graph.nodes.map(n => [n.id,n]));
  let selected = null, data, projected = [], scale = 1, pan = [0,0], rotation = [0.45,-0.2], drag = null;
  let width=1,height=1;
  const caption = (id) => label(byId.get(id) || {id});
  function choose(id) {
    selected=id; picker.value=String(id); hint.textContent=caption(id);
    update(); onNode(byId.get(id));
  }
  function update() {
    data=analyze(selected,Math.max(1,Math.min(100,Number(kInput.value)||10)),Number(threshold.value)||0);
    // Expose engine metadata with the canvas for inspection and accessibility tooling.
    root.vectorData=data;
    count.textContent=`${data.points.length} points`;
    nearest.replaceChildren();
    for (const row of data.nearest) {
      const li=document.createElement('li'); li.dataset.node=row.id; li.dataset.score=row.score;
      const button=document.createElement('button'); button.textContent=caption(row.id); button.onclick=()=>choose(row.id);
      const score=document.createElement('span'); score.textContent=row.score.toFixed(4); li.append(button,score); nearest.append(li);
    }
    const far=data.flags.filter(f=>f.kind==='linked-but-far').length;
    summary.textContent=`${far} linked-but-far · ${data.flags.length-far} near-but-unlinked`;
    flags.replaceChildren();
    if (linksToggle.checked) for (const flag of data.flags.slice(0,100)) {
      const li=document.createElement('li'); li.dataset.kind=flag.kind;
      li.textContent=`${caption(flag.from)} ↔ ${caption(flag.to)} · ${flag.kind} (${flag.score.toFixed(4)})`; flags.append(li);
    }
    if (linksToggle.checked && data.flags.length>100) { const li=document.createElement('li');li.textContent=`Showing 100 of ${data.flags.length} pairs; all pairs are highlighted on the canvas.`;flags.append(li); }
    draw();
  }
  function draw() {
    if (!data) return;
    const dark=theme==='dark';
    ctx.clearRect(0,0,width,height); ctx.fillStyle=dark?'#101d22':'#f8faf7';ctx.fillRect(0,0,width,height);
    const values=[...new Set(data.points.map(p=>String(byId.get(p.id)?.[colour.value]??'—')))].sort();
    const colours=new Map(values.map((v,i)=>[v,palette[i%palette.length]]));
    legend.replaceChildren();
    for (const value of values.slice(0,12)) { const item=document.createElement('span');item.textContent=value;item.style.color=colours.get(value);legend.append(item); }
    const extent=Math.max(0.000001,...data.points.flatMap(p=>p.position.map(Math.abs)));
    const factor=Math.min(width,height)*0.36/extent*scale;
    const nearestIds=new Set(data.nearest.map(n=>n.id));
    projected=data.points.map(p=>{
      let [x,y,z]=p.position;
      if(kind==='vector3d') { const a=rotation[0],b=rotation[1]; [x,z]=[x*Math.cos(a)+z*Math.sin(a),z*Math.cos(a)-x*Math.sin(a)];[y,z]=[y*Math.cos(b)-z*Math.sin(b),y*Math.sin(b)+z*Math.cos(b)]; }
      return {...p,x:width/2+x*factor+pan[0],y:height/2-y*factor+pan[1],z};
    }).sort((a,b)=>a.z-b.z||a.id-b.id);
    const positions=new Map(projected.map(p=>[p.id,p]));
    const edge=(from,to,color,dashed=false)=>{const a=positions.get(from),b=positions.get(to);if(!a||!b)return;ctx.beginPath();ctx.strokeStyle=color;ctx.lineWidth=1;ctx.setLineDash(dashed?[3,5]:[]);ctx.moveTo(a.x,a.y);ctx.lineTo(b.x,b.y);ctx.stroke();ctx.setLineDash([]);};
    if(linksToggle.checked) {
      for(const link of data.links)edge(link.from,link.to,dark?'#72878d':'#8c9f97');
      for(const flag of data.flags)edge(flag.from,flag.to,flag.kind==='linked-but-far'?'#d76a49':'#6599ba33',flag.kind==='near-but-unlinked');
    }
    if(selected!==null)for(const row of data.nearest)edge(selected,row.id,'#82b5a188',true);
    const query=search.value.toLowerCase();
    for(const p of projected) {
      const node=byId.get(p.id),isSelected=p.id===selected,isNear=nearestIds.has(p.id);
      const match=!query || Object.values(node).filter(v=>typeof v==='string').join(' ').toLowerCase().includes(query);
      ctx.globalAlpha=match?1:0.12;
      const r=isSelected?8:isNear?6:4.5;
      ctx.fillStyle=colours.get(String(node[colour.value]??'—'));ctx.beginPath();ctx.arc(p.x,p.y,r,0,Math.PI*2);ctx.fill();
      if(isSelected||isNear||query&&match){ctx.beginPath();ctx.arc(p.x,p.y,r+4,0,Math.PI*2);ctx.strokeStyle=isSelected?'#ffffff':ctx.fillStyle;ctx.lineWidth=1.5;ctx.stroke();}
      if(isSelected||isNear){ctx.fillStyle=dark?'#e0ede7':'#243c30';ctx.font='11px system-ui';ctx.fillText(caption(p.id).slice(0,34),p.x+11,p.y+3);}
    }
    ctx.globalAlpha=1;
    canvas.dataset.points=String(projected.length);
    canvas.projectedPoints=projected.map(({id,x,y})=>({id,x,y}));
  }
  canvas.onpointerdown=e=>{drag={x:e.clientX,y:e.clientY,moved:0};canvas.setPointerCapture(e.pointerId);};
  canvas.onpointermove=e=>{if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;drag.moved+=Math.abs(dx)+Math.abs(dy);drag.x=e.clientX;drag.y=e.clientY;if(kind==='vector3d'&&!e.shiftKey){rotation[0]+=dx/150;rotation[1]+=dy/150;}else{pan[0]+=dx;pan[1]+=dy;}draw();};
  canvas.onpointerup=e=>{if(drag&&drag.moved<5){const rect=canvas.getBoundingClientRect();const x=e.clientX-rect.left,y=e.clientY-rect.top;const p=[...projected].sort((a,b)=>Math.hypot(a.x-x,a.y-y)-Math.hypot(b.x-x,b.y-y))[0];if(p&&Math.hypot(p.x-x,p.y-y)<18)choose(p.id);}drag=null;};
  canvas.onwheel=e=>{e.preventDefault();scale=Math.max(0.15,Math.min(15,scale*Math.exp(-e.deltaY/600)));draw();};
  canvas.onkeydown=e=>{if(['+','=','-'].includes(e.key)){scale*=e.key==='-'?0.9:1.1;draw();}};
  reset.onclick=()=>{scale=1;pan=[0,0];rotation=[0.45,-0.2];draw();};
  colour.onchange=draw;search.oninput=draw;linksToggle.onchange=update;kInput.onchange=update;threshold.onchange=update;
  picker.onchange=()=>{if(picker.value)choose(Number(picker.value));};
  update();
  for(const p of data.points)picker.add(new Option(caption(p.id),String(p.id)));
  if(!data.points.length)hint.textContent='Select id in your query to plot its result nodes.';
  const resize=new ResizeObserver(()=>{const rect=canvas.getBoundingClientRect();width=rect.width;height=rect.height;const ratio=devicePixelRatio||1;canvas.width=Math.round(width*ratio);canvas.height=Math.round(height*ratio);ctx.setTransform(ratio,0,0,ratio,0,0);draw();});resize.observe(canvas);
  return ()=>{resize.disconnect();root.remove();};
}
