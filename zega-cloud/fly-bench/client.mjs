#!/usr/bin/env node
// The zega Fly Machines benchmark, run from a client Machine in the same
// region as the servers (so no Calgary network is in the numbers). Adapted
// from zega-cloud/containers-bench/driver/run.mjs; the load, read, write,
// hop2, heavy, snapshot and reload loops are that bench's src/bench.js.
//
// Every request goes through Fly's proxy over Flycast
// (http://<app>.flycast:<port>, one port per server Machine), because that is
// the path that counts as traffic for autostop and wakes a stopped Machine.
// Prints one JSON line per result, then Markdown tables.
//
//   node client.mjs --smoke                       # one Machine, tiny graph
//   node client.mjs                               # the full plan
//   node client.mjs --plan wake --wake s1x        # only the idle-stop wake
//
// Plan (sizes in --targets order; the first sets the shared size):
//   1. wipe; grow a graph x--factor per step up to the size's cap, each step
//      load + snapshot + reload, stopping before the next step is predicted
//      to pass --margin of the memory the machine offers. The last step is
//      the measured capacity; the capacity past the cap is an estimate from
//      memory per node.
//   2. read, write, hop2, heavy, snapshot, reload at the shared size and, for
//      the larger sizes, again at their own measured capacity.
//   3. wake: leave --wake idle for --idle-min, so Fly's proxy stops it
//      (autostop), then time the first answered query (autostart + reload of
//      the graph from the volume). --rounds times.
// Limits by default: sequential requests, at most --rps 20, --pause-ms
// between phases, caps per size in common.mjs. No push to failure.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { LIMITS, SIZES, MiB, PacedBench, budgetOf, count, estimateCapacity, flags, markdown, mb, ms, pair, pairs, sleep } from './common.mjs';

const o = flags(
  process.argv.slice(2),
  {
    app: 'zega-bench',
    targets: 's1x=8101,s2x=8102,s4x=8103,p1x=8104',
    tokenFile: '/etc/zega/admin-token',
    plan: 'all',
    shared: 0,
    start: 10000,
    factor: 1.25,
    batch: 1000,
    margin: 0.85,
    n: LIMITS.n,
    heavyN: LIMITS.heavyN,
    rps: LIMITS.maxRps,
    pauseMs: LIMITS.pauseMs,
    wake: 's1x',
    rounds: 3,
    idleMin: 10,
    out: '',
    holdMin: 0,
  },
  ['smoke'],
);
if (!['all', 'capacity', 'wake'].includes(o.plan)) throw new Error('--plan is all, capacity or wake');
if (o.rps > LIMITS.maxRps) throw new Error(`--rps is capped at ${LIMITS.maxRps}`);
const token = readFileSync(o.tokenFile, 'utf8').trim();
const targets = pairs(o.targets).map(([name, where]) => {
  if (!SIZES[name]) throw new Error(`unknown size ${name}; known: ${Object.keys(SIZES).join(', ')}`);
  const url = /^\d+$/.test(where) ? `http://${o.app}.flycast:${where}` : where.replace(/\/$/, '');
  return { name, url, size: SIZES[name], bench: new PacedBench({ url, token, rps: o.rps }) };
});

const results = [];
function emit(line) {
  const out = { at: new Date().toISOString(), ...line };
  results.push(out);
  console.log(JSON.stringify(out));
}
const pause = () => sleep(o.pauseMs);
// Every result names its Machine; `mem` bodies are trimmed to what the tables use.
const trim = mem => mem && { rss: mem.memory?.rss, peakRss: mem.memory?.peakRss, machine: mem.machine, disk: mem.disk, openMs: mem.openMs, startedAtMs: mem.startedAtMs };
async function attempt(t, phase, fn) {
  try {
    const result = await fn();
    const line = { phase, size: t.name, vm: t.size.vm, ...result };
    if (line.mem) line.mem = trim(line.mem);
    emit(line);
    return line;
  } catch (error) {
    const line = { phase, size: t.name, vm: t.size.vm, ok: false, error: error.message };
    emit(line);
    return line;
  }
}

async function loadTo(t, nodes) {
  let last;
  do {
    last = await attempt(t, 'load', () => t.bench.load({ nodes, batch: o.batch }));
  } while (last.ok && !last.done);
  return last;
}

// One step: load, snapshot, reload, with the peak RSS reset before each so
// each reports its own peak.
async function step(t, nodes) {
  await t.bench.mem(true);
  const load = await loadTo(t, nodes);
  if (!load.ok) return { ok: false, nodes, failed: 'load', error: load.error };
  const loadPeak = load.mem?.peakRss;
  const snapshot = await attempt(t, 'step-snapshot', () => t.bench.snapshot());
  if (!snapshot.ok) return { ok: false, nodes, failed: 'snapshot', error: snapshot.error };
  const reload = await attempt(t, 'step-reload', () => t.bench.reload());
  if (!reload.ok) return { ok: false, nodes, failed: 'reload', error: reload.error };
  const peaks = { load: loadPeak, snapshot: snapshot.after?.peakRss, reload: reload.after?.peakRss };
  return { ok: true, nodes, peaks, peak: Math.max(...Object.values(peaks).filter(v => v != null)), reloadMs: reload.serverMs };
}

const next = nodes => Math.max(nodes + o.batch, Math.round((nodes * o.factor) / o.batch) * o.batch);

async function probe(t, from, budget) {
  const cap = t.size.cap;
  let good = null;
  let nodes = Math.min(from, cap);
  for (;;) {
    const result = await step(t, nodes);
    emit({ phase: 'step', size: t.name, vm: t.size.vm, ...result, budget, cap });
    await pause();
    if (!result.ok) break;
    good = result;
    if (nodes >= cap) break;
    const upcoming = Math.min(next(nodes), cap);
    // Memory grows about linearly with nodes: stop before the next step is
    // predicted to pass the margin, so nothing is pushed into an OOM.
    if ((result.peak * upcoming) / nodes > o.margin * budget) break;
    nodes = upcoming;
  }
  return good;
}

async function benches(t, label) {
  const out = {};
  for (const [kind, n] of [['read', o.n], ['write', o.n], ['hop2', o.n], ['heavy', o.heavyN]]) {
    out[kind] = await attempt(t, label, () => t.bench.loop(kind, { n }));
    await pause();
  }
  for (const kind of ['snapshot', 'reload']) {
    out[kind] = await attempt(t, label, async () => ({ bench: kind, ...(await t.bench[kind]()) }));
    await pause();
  }
  return out;
}

async function capacityPlan(summary) {
  let shared = o.shared || null;
  for (const t of targets) {
    const s = (summary[t.name] = { vm: t.size.vm, cap: t.size.cap });
    await attempt(t, 'wipe', () => t.bench.wipe());
    const empty = await t.bench.mem(true);
    s.baseline = empty.memory?.rss;
    s.budget = budgetOf(empty);
    s.machine = empty.machine;
    emit({ phase: 'machine', size: t.name, vm: t.size.vm, budget: s.budget, baselineRss: s.baseline, machine: empty.machine, openMs: empty.openMs });
    if (shared == null) {
      s.capacity = await probe(t, o.start, s.budget);
      if (!s.capacity) throw new Error(`${t.name} failed its first step at ${o.start} nodes`);
      shared = s.capacity.nodes;
      emit({ phase: 'shared-size', size: t.name, nodes: shared, relationships: shared * 3 });
      s.shared = await benches(t, 'at-shared');
      s.own = s.shared;
    } else {
      const load = await loadTo(t, shared);
      if (load.ok) {
        await attempt(t, 'snapshot', () => t.bench.snapshot());
        s.shared = await benches(t, 'at-shared');
      }
      s.capacity = shared < t.size.cap ? await probe(t, next(shared), s.budget) : null;
      if (s.capacity) s.own = await benches(t, 'at-capacity');
    }
    if (s.capacity) {
      s.estimate = estimateCapacity({ nodes: s.capacity.nodes, peak: s.capacity.peak, baseline: s.baseline, budget: s.budget });
      emit({ phase: 'capacity-estimate', size: t.name, vm: t.size.vm, measuredNodes: s.capacity.nodes, cap: t.size.cap, ...s.estimate, note: 'extrapolated from memory per node; an estimate, not a measurement' });
    }
  }
  return shared;
}

async function wakePlan(wakes) {
  const t = targets.find(x => x.name === o.wake);
  if (!t) throw new Error(`--wake ${o.wake} is not in --targets`);
  if (!(await t.bench.store.get('loaded'))) await t.bench.findLoaded(o.batch);
  const before = await t.bench.fingerprint();
  for (let round = 1; round <= o.rounds; round++) {
    const awake = await t.bench.firstAnswer(180000);
    const m0 = await t.bench.mem();
    emit({ phase: 'wake-idle', size: t.name, round, idleMin: o.idleMin, startedAtMs: m0.startedAtMs, awakeMs: awake.ms });
    await sleep(o.idleMin * 60000);
    const line = await attempt(t, 'wake', async () => {
      const first = await t.bench.firstAnswer(180000);
      const m1 = await t.bench.mem();
      const warm = await t.bench.zql('{ Item(n: 1) { n } }');
      const after = await t.bench.fingerprint();
      return {
        ok: after.hash === before.hash,
        round,
        idleMin: o.idleMin,
        firstAnswerMs: first.ms,
        attempts: first.attempts,
        restarted: m1.startedAtMs !== m0.startedAtMs,
        openMs: m1.openMs,
        warmReadMs: Math.round(warm.ms * 100) / 100,
        nodes: after.nodes,
        graphIntact: after.hash === before.hash && after.found === before.found,
        mem: m1,
      };
    });
    wakes.push(line);
  }
}

function report(summary, wakes) {
  const columns = Object.keys(summary);
  const rows = [
    ['VM', c => summary[c].vm],
    ['memory the process can use (MiB)', c => mb(summary[c].budget)],
    ['cap for this run (nodes)', c => count(summary[c].cap)],
    ['measured capacity (nodes / relationships)', c => (summary[c].capacity ? `${count(summary[c].capacity.nodes)} / ${count(summary[c].capacity.nodes * 3)}` : '–')],
    ['peak RSS there: load / snapshot / reload (MiB)', c => (summary[c].capacity ? `${mb(summary[c].capacity.peaks.load)} / ${mb(summary[c].capacity.peaks.snapshot)} / ${mb(summary[c].capacity.peaks.reload)}` : '–')],
    ['memory per node, with 3 relationships (bytes)', c => count(summary[c].estimate?.bytesPerNode)],
    ['**estimated** capacity at 90% / 100% of memory (nodes)', c => (summary[c].estimate ? `${count(summary[c].estimate.at90)} / ${count(summary[c].estimate.at100)}` : '–')],
  ];
  for (const at of ['shared', 'own']) {
    rows.push([`**at ${at === 'shared' ? 'shared size' : 'own capacity'}** (nodes)`, c => count(summary[c][at]?.read?.nodes)]);
    for (const kind of ['read', 'write', 'hop2', 'heavy']) rows.push([`${kind} p50 / p99 (ms)`, c => pair(summary[c][at]?.[kind])]);
    rows.push(['snapshot (ms, server)', c => ms(summary[c][at]?.snapshot?.serverMs)]);
    rows.push(['reload from volume (ms, server)', c => ms(summary[c][at]?.reload?.serverMs)]);
  }
  let out = '';
  if (columns.length) out += `\nTimings: from a client Machine in the same region, through Fly's proxy over Flycast; ${o.n} samples each (heavy ${o.heavyN}), sequential, at most ${o.rps} requests/s.\n\n${markdown(columns, rows)}`;
  if (wakes.length) {
    out += `\nIdle wake (${o.wake}): left idle ${o.idleMin} min so Fly's proxy stops it, then the first query through Flycast (autostart, then the graph reloaded from the volume).\n\n`;
    out += markdown(wakes.map(w => `round ${w.round}`), [
      ['stopped by the proxy (process restarted)', c => String(wakes.find(w => `round ${w.round}` === c)?.restarted ?? '–')],
      ['first answer (ms)', c => ms(wakes.find(w => `round ${w.round}` === c)?.firstAnswerMs)],
      ['of which: open graph from volume (ms, server)', c => ms(wakes.find(w => `round ${w.round}` === c)?.openMs)],
      ['next read (ms)', c => ms(wakes.find(w => `round ${w.round}` === c)?.warmReadMs)],
      ['graph intact (nodes)', c => { const w = wakes.find(x => `round ${x.round}` === c); return w ? `${w.graphIntact} (${count(w.nodes)})` : '–'; }],
    ]);
  }
  return out;
}

async function smoke() {
  const t = targets[0];
  const lines = [];
  const run = async (phase, fn) => lines.push(await attempt(t, phase, fn));
  await run('smoke-wipe', () => t.bench.wipe());
  await run('smoke-mem', async () => ({ ok: true, mem: await t.bench.mem() }));
  await run('smoke-load', () => t.bench.load({ nodes: 2000, batch: o.batch }));
  for (const [kind, n] of [['read', 5], ['write', 5], ['hop2', 5], ['heavy', 2]]) await run('smoke', () => t.bench.loop(kind, { n }));
  await run('smoke', async () => ({ bench: 'snapshot', ...(await t.bench.snapshot()) }));
  await run('smoke', async () => ({ bench: 'reload', ...(await t.bench.reload()) }));
  await run('smoke-fingerprint', async () => ({ ok: true, ...(await t.bench.fingerprint()) }));
  const denied = await fetch(`${t.url}/zql`, { method: 'POST', body: '{}' }).then(r => r.status).catch(e => e.message);
  lines.push({ ok: denied === 401 });
  emit({ phase: 'smoke-auth', size: t.name, ok: denied === 401, withoutTokenStatus: denied });
  await run('smoke-wipe', () => t.bench.wipe());
  const failed = lines.filter(l => !l.ok).length;
  console.log(`\nsmoke on ${t.name} (${t.url}): ${failed === 0 ? 'ok' : `${failed} failed`}`);
  return failed === 0;
}

async function main() {
  const started = Date.now();
  const summary = {};
  const wakes = [];
  let ok = true;
  try {
    if (o.smoke) ok = await smoke();
    else {
      if (o.plan !== 'wake') await capacityPlan(summary);
      if (o.plan !== 'capacity' && o.wake) await wakePlan(wakes);
    }
  } catch (error) {
    ok = false;
    emit({ phase: 'fatal', ok: false, error: error.message });
  }
  const table = o.smoke ? '' : report(summary, wakes);
  console.log(table);
  console.log(`elapsed ${Math.round((Date.now() - started) / 1000)} s`);
  if (o.out) {
    mkdirSync(o.out, { recursive: true });
    const stem = join(o.out, `fly-bench-${new Date(started).toISOString().replace(/[:.]/g, '-')}`);
    writeFileSync(`${stem}.jsonl`, results.map(r => JSON.stringify(r)).join('\n') + '\n');
    writeFileSync(`${stem}.md`, table);
    console.log(`wrote ${stem}.jsonl and ${stem}.md`);
  }
  if (o.holdMin > 0) {
    console.log(`holding ${o.holdMin} min for flyctl ssh sftp get`);
    await sleep(o.holdMin * 60000);
  }
  process.exit(ok ? 0 : 1);
}

void MiB;
main();
