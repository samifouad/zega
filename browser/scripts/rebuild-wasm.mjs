import { execFileSync } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { hashFiles } from './wasm-files.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const engineArg = process.argv[2];
if (!engineArg || process.argv.length !== 3) {
  console.error('Usage: npm run wasm:rebuild -- /absolute/path/to/zega');
  process.exit(1);
}
const engine = resolve(engineArg);
const git = (...args) => execFileSync('git', ['-C', engine, ...args], { encoding: 'utf8' }).trim();
await access(join(engine, 'zega-wasm/Cargo.toml'));
await access(join(engine, 'zega-wasm/Cargo.lock'));
if (git('status', '--porcelain')) throw new Error('Engine checkout must be clean so the recorded commit identifies the build inputs.');
const commit = git('rev-parse', 'HEAD');
await mkdir(join(root, '.tmp'), { recursive: true, mode: 0o700 });
const staging = await mkdtemp(join(root, '.tmp/wasm-'));
const output = join(staging, 'pkg');
try {
  const tool = (command) => execFileSync(command, ['--version'], { encoding: 'utf8' }).trim();
  const toolchain = { rustc: tool('rustc'), wasmPack: tool('wasm-pack') };
  execFileSync('wasm-pack', ['build', join(engine, 'zega-wasm'), '--target', 'web', '--release', '--out-dir', output, '--locked'], {
    cwd: root,
    stdio: 'inherit',
    env: { ...process.env, CARGO_TARGET_DIR: join(root, '.target/wasm'), TMPDIR: join(root, '.tmp') },
  });
  // wasm-pack ignores generated files by default; these files are vendored here.
  await rm(join(output, '.gitignore'), { force: true });
  for (const name of ['package.json', 'zega_wasm.js', 'zega_wasm_bg.wasm', 'zega_wasm.d.ts', 'zega_wasm_bg.wasm.d.ts']) await access(join(output, name));
  await WebAssembly.compile(await readFile(join(output, 'zega_wasm_bg.wasm')));
  if (git('rev-parse', 'HEAD') !== commit || git('status', '--porcelain')) throw new Error('Engine changed during the build; pkg/ was not replaced.');
  const metadata = {
    repository: 'https://github.com/zegadb/zega',
    commit,
    provenance: 'Rebuilt from a clean local engine checkout with wasm-pack --target web --release --locked.',
    toolchain,
    sha256: await hashFiles(output),
  };
  await writeFile(join(staging, 'wasm-source.json'), JSON.stringify(metadata, null, 2) + '\n');
  await rename(join(root, 'pkg'), join(staging, 'previous-pkg'));
  try {
    await rename(output, join(root, 'pkg'));
    await rename(join(staging, 'wasm-source.json'), join(root, 'wasm-source.json'));
  } catch (error) {
    await rm(join(root, 'pkg'), { recursive: true, force: true });
    await rename(join(staging, 'previous-pkg'), join(root, 'pkg'));
    throw error;
  }
  console.log(`Vendored wasm from engine ${commit}. Run npm test and commit pkg/ with wasm-source.json.`);
} finally {
  await rm(staging, { recursive: true, force: true });
}
