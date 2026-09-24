import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { writeFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { corpusPath, revision } from './fetch-corpus.mjs';
import { root, localServer } from './local-server.mjs';
import { control } from '../bench/common.mjs';
const directory = await corpusPath();
const { loadCorpus, grade, compare, corpusDigest } = await import(pathToFileURL(resolve(directory, 'scripts/corpus.mjs')));
const corpus = loadCorpus();
let server;
const endpoint = process.argv[2] ?? (server = await localServer({ port: 8800, name: 'corpus' })).endpoint;
const results = [];
const counts = { PASS: 0, FAIL: 0, XFAIL: 0, XPASS: 0, SKIP: 0, parity: 0 };
const run = randomUUID();
try {
  for (const [i, test] of corpus.entries()) {
    if (test.network) { counts.SKIP++; results.push({ id: test.id, status: 'SKIP', reason: 'remote import is opt-in in upstream corpus; local spike run is offline' }); continue; }
    const sources = Object.fromEntries(Object.entries(test.files).flatMap(([name, text]) => [[name, text], [`./${name}`, text]]));
    const nativeInput = { source: test.source, api: test.api, sources };
    const native = spawnSync(resolve(root, '.target/debug/cloud-native'), [], { input: JSON.stringify(nativeInput) + '\n', encoding: 'utf8', maxBuffer: 4 * 1024 * 1024, timeout: 30000 });
    if (native.status !== 0 || native.error || native.stderr) throw new Error(`native ${test.id}: ${native.error ?? native.stderr}`);
    const expected = JSON.parse(native.stdout);
    const graph = `corpus-${run}-${i}`;
    const parseResponse = await fetch(`${endpoint}/g/${graph}/_spike-check`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(nativeInput), signal: AbortSignal.timeout(30000) });
    if (!parseResponse.ok) throw new Error(`${test.id}: parser endpoint HTTP ${parseResponse.status}`);
    const check = await parseResponse.json();
    if (!check.ok) throw new Error(`parser host failure: ${JSON.stringify(check)}`);
    let actual;
    if (check.result.error !== null) actual = { ok: false, stage: 'parse', stdout: '', stderr: check.result.error + '\n' };
    else {
      const response = await fetch(`${endpoint}/g/${graph}/zql`, { method: 'POST', headers: { 'content-type': 'application/json', 'Request-Id': `case-${i}` }, body: JSON.stringify({ query: test.source, document: true, sources }), signal: AbortSignal.timeout(30000) });
      if (![200, 400].includes(response.status)) throw new Error(`${test.id}: endpoint infrastructure HTTP ${response.status}: ${await response.text()}`);
      const body = await response.text();
      const value = JSON.parse(body);
      const prefix = '{"ok":true,"result":';
      if (value.ok && (!body.startsWith(prefix) || !body.endsWith('}'))) throw new Error('unexpected native JSON envelope');
      // Retain Rust's exact number spelling (1.0 vs 1). JSON.parse/stringify
      // would erase this distinction and corrupt the corpus comparison.
      actual = value.ok ? { ok: true, stage: 'run', stdout: body.slice(prefix.length, -1) + '\n', stderr: '' }
        : { ok: false, stage: 'run', stdout: '', stderr: value.error + '\n' };
    }
    const differences = compare(actual, expected);
    const verdict = grade(test, actual);
    if (!differences.length) counts.parity++;
    counts[verdict.status]++;
    results.push({ id: test.id, ...verdict, parity: differences.length === 0, native: expected, actual });
    if (differences.length || ['FAIL', 'XPASS'].includes(verdict.status)) console.error(JSON.stringify(results.at(-1)));
    // Free graph memory without losing persisted state, using actual DO eviction.
    try { await control(endpoint, graph, 'evict'); } catch {}
  }
} finally { await server?.stop(); }
const report = { corpus: 'zegadb/testsuite', revision, digest: corpusDigest(corpus), endpoint, counts, total: corpus.length, results };
await mkdir(resolve(root, '.tmp/evidence'), { recursive: true });
await writeFile(resolve(root, '.tmp/evidence/corpus.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify({ ...report, results: undefined }));
if (counts.FAIL || counts.XPASS || counts.parity + counts.SKIP !== corpus.length) process.exitCode = 1;
