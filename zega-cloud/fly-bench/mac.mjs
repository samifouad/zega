#!/usr/bin/env node
// The zega Fly benchmark's measurements taken from Sami's Mac: what a remote
// user feels, and the ones that need flyctl (stop/start and resize). Ava runs
// this; it calls flyctl, so it must run where flyctl is logged in.
// Requests go to https://<app>.fly.dev through Fly's proxy (shared IPv4),
// pinned to one Machine with the `fly-force-instance-id` header.
//
//   node mac.mjs e2e      --machines s1x=<id>,s2x=<id>,s4x=<id>,p1x=<id>
//   node mac.mjs restart  --machines ...           # stop -> start -> first answer, graph on the volume
//   node mac.mjs restart  --machines ... --empty   # wipes each graph first: the empty-volume restart
//   node mac.mjs resize   --machine s1x=<id>       # 1x/256 -> 2x/512 -> 4x/1024 -> back to 1x/256
//
// Limits by default: sequential requests, --n 30 samples per size spaced
// --gap-ms 500 apart; the availability poll during a restart or resize sends
// one request every --poll-ms 250.
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { promisify } from 'node:util';
import { LIMITS, SIZES, PacedBench, count, flags, markdown, ms, pair, pairs, sleep } from './common.mjs';

const run = promisify(execFile);
const [command, ...rest] = process.argv.slice(2);
const o = flags(
  rest,
  {
    app: 'zega-bench',
    base: '',
    machines: '',
    machine: '',
    tokenFile: './admin-token',
    flyctl: 'flyctl',
    n: LIMITS.macN,
    gapMs: LIMITS.macGapMs,
    pollMs: 250,
    rounds: 2,
    steps: 'shared-cpu-2x:512,shared-cpu-4x:1024,shared-cpu-1x:256',
    pauseMs: LIMITS.pauseMs,
  },
  ['empty'],
);
const token = readFileSync(o.tokenFile, 'utf8').trim();
const base = (o.base || `https://${o.app}.fly.dev`).replace(/\/$/, '');
const machines = pairs(o.machines || o.machine).map(([name, id]) => {
  if (!SIZES[name]) throw new Error(`unknown size ${name}`);
  return { name, id, vm: SIZES[name].vm, bench: bench(id, 1000 / o.gapMs) };
});
if (machines.length === 0) throw new Error('pass --machines name=<machine id>,...');

function bench(id, rps, timeoutMs = 30000) {
  return new PacedBench({ url: base, token, rps: Math.min(rps, LIMITS.maxRps), headers: { 'fly-force-instance-id': id }, timeoutMs });
}

function emit(line) {
  console.log(JSON.stringify({ at: new Date().toISOString(), ...line }));
  return line;
}

async function flyctl(...argv) {
  const start = performance.now();
  try {
    await run(o.flyctl, [...argv, '--app', o.app], { maxBuffer: 1 << 20 });
    return { ok: true, ms: Math.round(performance.now() - start) };
  } catch (error) {
    return { ok: false, ms: Math.round(performance.now() - start), error: (error.stderr || error.message).trim().slice(0, 300) };
  }
}

/**
 * Polls one query every --poll-ms from `since` until one answers from a
 * process started after `startedAfter`. Returns when, and the longest run of
 * failures seen (the time nothing answered).
 */
async function watch(m, { since, startedAfter, until = () => false, timeoutMs = 300000 }) {
  const probe = bench(m.id, 1000 / o.pollMs, 5000);
  let lastOk = since;
  let firstFail = null;
  let longestGap = 0;
  let attempts = 0;
  while (performance.now() - since < timeoutMs) {
    attempts++;
    const at = performance.now();
    try {
      const { body } = await probe.json('/mem');
      if (firstFail != null) longestGap = Math.max(longestGap, at - lastOk);
      firstFail = null;
      lastOk = at;
      if (body.startedAtMs > startedAfter) return { answeredMs: Math.round(at - since), unavailableMs: Math.round(longestGap), attempts, mem: body };
      if (until()) return { answeredMs: null, unavailableMs: Math.round(longestGap), attempts, mem: body, note: 'no restart seen' };
    } catch {
      firstFail ??= at;
    }
  }
  throw new Error(`no answer from a restarted process within ${timeoutMs} ms`);
}

async function e2e() {
  const summary = {};
  for (const m of machines) {
    const nodes = await m.bench.findLoaded();
    await m.bench.store.put('writes', 10_000_000 + (Math.floor(Date.now() / 1000) % 1_000_000) * 100);
    const s = (summary[m.name] = { nodes });
    for (const kind of ['read', 'write', 'hop2']) {
      s[kind] = emit({ phase: 'e2e', size: m.name, vm: m.vm, ...(await m.bench.loop(kind, { n: o.n })) });
      await sleep(o.pauseMs);
    }
  }
  const cols = Object.keys(summary);
  console.log(`\nEnd to end from this Mac to ${base} (Fly proxy, TLS, pinned per Machine); ${o.n} samples each, one every ${o.gapMs} ms.\n`);
  console.log(markdown(cols, [['VM', c => SIZES[c].vm], ['graph (nodes)', c => count(summary[c].nodes)], ...['read', 'write', 'hop2'].map(k => [`${k} p50 / p99 (ms)`, c => pair(summary[c][k])])]));
}

async function restart() {
  const summary = {};
  for (const m of machines) {
    await m.bench.findLoaded();
    if (o.empty) {
      emit({ phase: 'wipe', size: m.name, ...(await m.bench.wipe()) });
      await m.bench.store.put('loaded', 0);
    }
    const before = await m.bench.fingerprint();
    const rounds = (summary[m.name] = []);
    for (let round = 1; round <= o.rounds; round++) {
      const { body: mem0 } = await m.bench.json('/mem');
      const stop = await flyctl('machine', 'stop', m.id);
      if (!stop.ok) {
        rounds.push(emit({ phase: 'restart', size: m.name, round, ok: false, stop }));
        continue;
      }
      const since = performance.now();
      const start = await flyctl('machine', 'start', m.id);
      const seen = await watch(m, { since, startedAfter: mem0.startedAtMs }).catch(error => ({ error: error.message }));
      const after = seen.error ? null : await m.bench.fingerprint();
      rounds.push(
        emit({
          phase: 'restart',
          size: m.name,
          vm: m.vm,
          round,
          empty: o.empty,
          ok: !seen.error && start.ok && after.hash === before.hash,
          stopMs: stop.ms,
          startCommandMs: start.ms,
          firstAnswerMs: seen.answeredMs,
          openMs: seen.mem?.openMs,
          attempts: seen.attempts,
          graphIntact: after ? after.hash === before.hash && after.found === before.found : false,
          nodes: before.nodes,
          error: seen.error ?? start.error,
        }),
      );
      await sleep(o.pauseMs);
    }
  }
  const cols = Object.keys(summary);
  const pick = (c, f) => summary[c].map(f).join(', ');
  console.log(`\nStop -> start -> first answer, timed from this Mac (${o.empty ? 'empty volume' : 'graph reloaded from the volume'}); ${o.rounds} rounds each.\n`);
  console.log(
    markdown(cols, [
      ['VM', c => SIZES[c].vm],
      ['graph (nodes)', c => count(summary[c][0]?.nodes)],
      ['`flyctl machine stop` (ms)', c => pick(c, r => ms(r.stopMs))],
      ['`flyctl machine start` returns (ms)', c => pick(c, r => ms(r.startCommandMs))],
      ['start issued -> first answer (ms)', c => pick(c, r => ms(r.firstAnswerMs))],
      ['of which: open graph from volume (ms, server)', c => pick(c, r => ms(r.openMs))],
      ['graph intact', c => pick(c, r => String(r.graphIntact))],
    ]),
  );
}

async function resize() {
  const m = machines[0];
  await m.bench.findLoaded();
  const before = await m.bench.fingerprint();
  emit({ phase: 'resize-before', size: m.name, ...before });
  const rows = [];
  for (const target of o.steps.split(',')) {
    const [vm, memory] = target.split(':');
    const { body: mem0 } = await m.bench.json('/mem');
    const since = performance.now();
    let done = false;
    const watching = watch(m, { since, startedAfter: mem0.startedAtMs, until: () => done });
    const update = await flyctl('machine', 'update', m.id, '--vm-size', vm, '--vm-memory', memory, '--yes');
    done = true;
    const seen = await watching.catch(error => ({ error: error.message }));
    const after = seen.error ? null : await m.bench.fingerprint();
    const read = seen.error ? null : await m.bench.loop('read', { n: 10 });
    rows.push(
      emit({
        phase: 'resize',
        size: m.name,
        to: `${vm}/${memory}MB`,
        ok: update.ok && !seen.error && after?.hash === before.hash,
        updateCommandMs: update.ms,
        firstAnswerMs: seen.answeredMs,
        unavailableMs: seen.unavailableMs,
        openMs: seen.mem?.openMs,
        memTotal: seen.mem?.machine?.memTotal,
        graphIntact: after ? after.hash === before.hash && after.found === before.found : false,
        nodes: before.nodes,
        readAfter: read && { p50Ms: read.p50Ms, p99Ms: read.p99Ms },
        error: seen.error ?? update.error ?? seen.note,
      }),
    );
    await sleep(o.pauseMs);
  }
  console.log(`\nResize of ${m.name} (${count(before.nodes)} nodes on its volume), timed from this Mac; unavailable = longest run of failed polls (one every ${o.pollMs} ms).\n`);
  console.log(
    markdown(
      rows.map(r => r.to),
      [
        ['`flyctl machine update` returns (ms)', c => ms(rows.find(r => r.to === c).updateCommandMs)],
        ['update issued -> first answer (ms)', c => ms(rows.find(r => r.to === c).firstAnswerMs)],
        ['unavailable (ms)', c => ms(rows.find(r => r.to === c).unavailableMs)],
        ['of which: open graph from volume (ms, server)', c => ms(rows.find(r => r.to === c).openMs)],
        ['MemTotal after (MiB)', c => ms((rows.find(r => r.to === c).memTotal ?? 0) / 1048576)],
        ['graph intact', c => String(rows.find(r => r.to === c).graphIntact)],
        ['read p50 / p99 after (ms)', c => pair(rows.find(r => r.to === c).readAfter)],
      ],
    ),
  );
}

const commands = { e2e, restart, resize };
if (!commands[command]) {
  console.error('usage: node mac.mjs e2e|restart|resize --machines name=<id>,... [--token-file ./admin-token]');
  process.exit(2);
}
commands[command]().catch(error => {
  console.error(error);
  process.exit(1);
});
