import { execFileSync } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { resolve, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { encodedRustflags, hashFiles, hostPaths, remapFlags } from './wasm-files.mjs';

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
  // Panic locations embed source paths. Remap every build-machine prefix so the
  // shipped wasm names no user, home or worktree (#45).
  const targetDir = resolve(process.env.CARGO_TARGET_DIR || join(root, '../.target'));
  const local = {
    engine,
    targetDir,
    cargoHome: resolve(process.env.CARGO_HOME || join(homedir(), '.cargo')),
    sysroot: execFileSync('rustc', ['--print', 'sysroot'], { encoding: 'utf8' }).trim(),
  };
  execFileSync('wasm-pack', ['build', join(engine, 'zega-wasm'), '--target', 'web', '--release', '--out-dir', output, '--locked'], {
    cwd: root,
    stdio: 'inherit',
    env: {
      ...process.env,
      CARGO_TARGET_DIR: targetDir,
      TMPDIR: process.env.TMPDIR || join(root, '.tmp'),
      CARGO_ENCODED_RUSTFLAGS: encodedRustflags(process.env, remapFlags(local)),
    },
  });
  // wasm-pack ignores generated files by default; these files are vendored here.
  await rm(join(output, '.gitignore'), { force: true });
  for (const name of ['package.json', 'zega_wasm.js', 'zega_wasm_bg.wasm', 'zega_wasm.d.ts', 'zega_wasm_bg.wasm.d.ts']) await access(join(output, name));
  const wasm = await readFile(join(output, 'zega_wasm_bg.wasm'));
  await WebAssembly.compile(wasm);
  const leaked = hostPaths(wasm, [...Object.values(local), homedir()]);
  if (leaked.length) throw new Error(`The build still embeds build-machine paths; pkg/ was not replaced:\n${leaked.slice(0, 10).join('\n')}`);
  if (git('rev-parse', 'HEAD') !== commit || git('status', '--porcelain')) throw new Error('Engine changed during the build; pkg/ was not replaced.');
  const { default: init, format_json } = await import(pathToFileURL(join(output, 'zega_wasm.js')));
  await init({ module_or_path: await readFile(join(output, 'zega_wasm_bg.wasm')) });
  await writeFile(join(output, 'package.json'), format_json(await readFile(join(output, 'package.json'), 'utf8')));
  const metadata = {
    repository: 'https://github.com/zegadb/zega',
    commit,
    provenance: 'Rebuilt from a clean local engine checkout with wasm-pack --target web --release --locked, with build-machine paths remapped to /zega, /target, /cargo and /rust.',
    toolchain,
    sha256: await hashFiles(output),
  };
  await writeFile(join(staging, 'wasm-source.json'), format_json(JSON.stringify(metadata)));
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
