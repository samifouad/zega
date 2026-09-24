import { mkdir, writeFile, access } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { root } from './local-server.mjs';
export const revision = 'd9684641b3a77f772f920c47bb40cc7d5b0e2995';
export async function corpusPath() {
  const dir = resolve(root, `.tmp/corpus-${revision}`);
  try { await access(resolve(dir, 'scripts/corpus.mjs')); return dir; } catch {}
  await mkdir(dir, { recursive: true });
  const response = await fetch(`https://codeload.github.com/zegadb/testsuite/tar.gz/${revision}`);
  if (!response.ok) throw new Error(`corpus download: ${response.status}`);
  const archive = resolve(dir, 'corpus.tar.gz');
  await writeFile(archive, new Uint8Array(await response.arrayBuffer()));
  const child = spawnSync('tar', ['-xzf', archive, '--strip-components=1', '-C', dir], { encoding: 'utf8' });
  if (child.status !== 0) throw new Error(child.stderr);
  return dir;
}
if (process.argv[1]?.endsWith('fetch-corpus.mjs')) console.log(await corpusPath());
