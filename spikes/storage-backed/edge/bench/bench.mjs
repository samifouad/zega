// SPIKE benchmark driver (Node 18+, no dependencies). Runs from any machine
// against the deployed spike Workers and writes one JSON file.
//
//   node bench/bench.mjs --do https://zega-storage-spike.<acct>.workers.dev \
//                        --d1 https://zega-storage-spike-d1.<acct>.workers.dev \
//                        --out results/run-1.json
//
// Options: --sizes 1000,10000,50000  --reps 100  --cold 10  --tag <graph-name-prefix>
// Either --do or --d1 may be omitted to run one target only.
//
// For each target and graph size: seed the graph, record its storage size,
// then time sequential requests per query shape (client end to end, and the
// server-side time the Worker saw), and what each request was billed
// (rows read/written). The Durable Object target also measures cold starts:
// it evicts the object, then times the first request.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith("--") ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const sizes = (args.sizes ?? "1000,10000,50000").split(",").map(Number);
const reps = Number(args.reps ?? 100);
const coldReps = Number(args.cold ?? 10);
const tag = args.tag ?? `b${Date.now().toString(36)}`;
const out = args.out ?? `results/bench-${tag}.json`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let seedState = 0x2545f491;
const rand = (n) => {
  seedState = (seedState * 1103515245 + 12345) >>> 0;
  return seedState % n;
};

async function call(url, method = "GET") {
  for (let attempt = 0; ; attempt++) {
    const t = performance.now();
    let res;
    try {
      res = await fetch(url, { method });
    } catch (e) {
      if (attempt < 3) { await sleep(500); continue; }
      throw e;
    }
    const text = await res.text();
    const ms = performance.now() - t;
    let body = null;
    try { body = JSON.parse(text); } catch { body = { ok: false, error: text.slice(0, 200) }; }
    const server = Number(res.headers.get("x-do-ms") ?? res.headers.get("x-d1-ms") ?? NaN);
    const colo = (res.headers.get("cf-ray") ?? "").split("-")[1] ?? null;
    if (res.status >= 500 && attempt < 2 && !url.includes("/evict")) { await sleep(300); continue; }
    return { status: res.status, ms, server, colo, body };
  }
}

function summarize(samples) {
  const pct = (xs, p) => {
    const s = [...xs].sort((a, b) => a - b);
    return s.length ? Math.round(s[Math.min(s.length - 1, Math.floor(s.length * p))] * 100) / 100 : null;
  };
  const avg = (xs) => (xs.length ? Math.round((xs.reduce((a, b) => a + b, 0) / xs.length) * 100) / 100 : null);
  const ok = samples.filter((s) => s.body?.ok);
  const client = ok.map((s) => s.ms);
  const server = ok.map((s) => s.server).filter((x) => !Number.isNaN(x));
  const cost = (k) => avg(ok.map((s) => s.body.cost?.[k] ?? 0));
  return {
    n: samples.length,
    errors: samples.length - ok.length,
    first_error: samples.find((s) => !s.body?.ok)?.body?.error ?? null,
    client_ms: { p50: pct(client, 0.5), p95: pct(client, 0.95), p99: pct(client, 0.99), mean: avg(client) },
    server_ms: { p50: pct(server, 0.5), p95: pct(server, 0.95), p99: pct(server, 0.99), mean: avg(server) },
    per_request: {
      billed_rows_read: cost("billed_rows_read"),
      billed_rows_written: cost("billed_rows_written"),
      statements: cost("statements"),
      round_trips: cost("round_trips"),
      cache_hits: cost("cache_hits"),
      cache_misses: cost("cache_misses"),
    },
    heap_peak_bytes: Math.max(0, ...ok.map((s) => s.body.heap?.peak ?? 0)),
    colo: ok[0]?.colo ?? null,
  };
}

const READS = ["point", "one_hop", "two_hop", "filter", "scan_limit", "scan_all"];

function param(shape, n) {
  if (shape === "filter") return rand(Math.max(Math.floor(n / 10), 1));
  if (shape === "scan_limit") return rand(100);
  return 1 + rand(n);
}

async function measureReads(base, n, extra = "") {
  const shapes = {};
  for (const shape of READS) {
    const count = shape === "scan_all" ? Math.min(reps, 10) : reps;
    for (let w = 0; w < 5; w++) await call(`${base}/q?shape=${shape}&i=${param(shape, n)}&n=${n}${extra}`);
    const samples = [];
    for (let r = 0; r < count; r++) samples.push(await call(`${base}/q?shape=${shape}&i=${param(shape, n)}&n=${n}${extra}`));
    shapes[shape] = summarize(samples);
    log(`    ${shape.padEnd(10)} p50 ${shapes[shape].client_ms.p50} ms (server ${shapes[shape].server_ms.p50}) rows ${shapes[shape].per_request.billed_rows_read}`);
  }
  return shapes;
}

async function measureWrites(base, n, offset) {
  const create = [], link = [];
  for (let r = 0; r < reps; r++) {
    const k = offset + r;
    create.push(await call(`${base}/q?shape=create&i=${k}&n=${n}`));
    link.push(await call(`${base}/q?shape=link&i=${k}&n=${n}`));
  }
  const res = { create: summarize(create), link: summarize(link) };
  log(`    create p50 ${res.create.client_ms.p50} ms, link p50 ${res.link.client_ms.p50} ms`);
  return res;
}

const log = (s) => console.error(s);

async function seed(base, n, chunk) {
  const t = performance.now();
  let written = 0;
  for (let lo = 1; lo <= n; lo += chunk) {
    const hi = Math.min(lo + chunk - 1, n);
    const r = await call(`${base}/seed?lo=${lo}&hi=${hi}&n=${n}`, "POST");
    if (!r.body?.ok) throw new Error(`seed ${lo}-${hi}: ${JSON.stringify(r.body)}`);
    written += r.body.cost?.billed_rows_written ?? 0;
  }
  return { seconds: Math.round((performance.now() - t) / 100) / 10, billed_rows_written: written, chunk };
}

async function runDo(url) {
  const out = {};
  for (const n of sizes) {
    const base = `${url}/g/${tag}do${n}`;
    log(`DO ${n}: seeding`);
    const seeded = await seed(base, n, 1000);
    const stats = (await call(`${base}/stats`)).body.result;
    log(`  ${JSON.stringify(stats)}`);
    log(`  warm reads`);
    const reads = await measureReads(base, n);
    log(`  writes`);
    const writes = await measureWrites(base, n, 10_000_000);
    log(`  cold`);
    const cold = { point: [], two_hop: [] };
    for (let r = 0; r < coldReps; r++) {
      for (const shape of ["point", "two_hop"]) {
        await call(`${base}/evict`, "POST");
        await sleep(1000);
        const s = await call(`${base}/q?shape=${shape}&i=${1 + rand(n)}&n=${n}`);
        s.cold = s.body?.opened === true;
        cold[shape].push(s);
      }
    }
    const coldSummary = Object.fromEntries(
      Object.entries(cold).map(([k, v]) => [k, { ...summarize(v), confirmed_cold: v.filter((s) => s.cold).length }]),
    );
    log(`  cold point p50 ${coldSummary.point.client_ms.p50} ms (${coldSummary.point.confirmed_cold}/${coldReps} confirmed)`);
    const after = (await call(`${base}/stats`)).body.result;
    out[n] = { seed: seeded, storage: stats, storage_after_writes: after, reads, writes, cold: coldSummary };
  }
  return out;
}

async function runD1(url) {
  const out = {};
  for (const n of sizes) {
    const g = `${tag}d${n}`.replace(/[^a-z0-9]/g, "");
    const base = `${url}/d1/${g}`;
    log(`D1 ${n}: seeding`);
    const seeded = await seed(base, n, 500);
    const stats = (await call(`${base}/stats`)).body.result;
    log(`  ${JSON.stringify(stats)}`);
    log(`  engine-shaped reads (one D1 call per storage call)`);
    const engine = await measureReads(base, n, "&mode=engine");
    log(`  compiled reads (one SQL statement per query)`);
    const compiled = await measureReads(base, n, "&mode=compiled");
    log(`  writes`);
    const writes = await measureWrites(base, n, 10_000_000);
    out[n] = { seed: seeded, storage: stats, reads_engine: engine, reads_compiled: compiled, writes };
  }
  return out;
}

const result = {
  spike: "zegadb/aps storage-backed zega",
  started: new Date().toISOString(),
  tag,
  sizes,
  reps,
  cold_reps: coldReps,
  node: process.version,
  targets: {},
};
if (args.do) result.targets.durable_object = await runDo(args.do.replace(/\/$/, ""));
if (args.d1) result.targets.d1 = await runD1(args.d1.replace(/\/$/, ""));
result.finished = new Date().toISOString();
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(result, null, 2));
log(`wrote ${out}`);
