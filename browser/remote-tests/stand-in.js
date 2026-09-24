// A local stand-in for the zega-containers-bench Worker: `/c/<size>/<graph>/…`
// is forwarded to a real `zega start --token-file`, which checks the Bearer
// token exactly as the server in the container does when it has one. CORS
// answers only local origins, like the Worker's dev allowance. Every request
// is recorded so tests can assert what the browser sent.
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';

const SIZES = ['lite', 'basic', 'std1'];
const LOCAL = /^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?$/;

async function startZega(directory, tokenFile) {
  const child = spawn(resolve('../.target/debug/zega'), ['start', '--port', '0', '--data', `${directory}/data`, '--token-file', tokenFile], { stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (data) => { stderr += data; });
  const url = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { child.kill(); reject(Error(`zega start timed out: ${stderr}`)); }, 15000);
    child.once('error', (error) => { clearTimeout(timeout); reject(error); });
    child.once('exit', (code) => { clearTimeout(timeout); reject(Error(`zega start exited ${code}: ${stderr}`)); });
    createInterface({ input: child.stdout }).once('line', (line) => { clearTimeout(timeout); resolve(line); });
  });
  return { url, async stop() { if (child.exitCode !== null) return; const exit = new Promise((r) => child.once('exit', r)); child.kill(); await exit; } };
}

async function body(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return Buffer.concat(chunks);
}

export async function startRemote(token) {
  await mkdir('.tmp', { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(resolve('.tmp/remote-'));
  const tokenFile = `${directory}/token`;
  await writeFile(tokenFile, token, { mode: 0o600 });
  const zega = await startZega(directory, tokenFile);
  const requests = [];
  const remote = { requests, delayNextMs: 0 };
  const server = createServer(async (request, response) => {
    const url = new URL(request.url, 'http://stand-in');
    const origin = request.headers.origin;
    const cors = origin && LOCAL.test(origin) ? { 'access-control-allow-origin': origin, vary: 'Origin' } : {};
    if (request.method === 'OPTIONS') {
      response.writeHead(204, { ...cors, 'access-control-allow-methods': 'GET, POST, DELETE', 'access-control-allow-headers': 'authorization, content-type', 'access-control-max-age': '7200' });
      return response.end();
    }
    const payload = await body(request);
    const [prefix, size, graph, ...rest] = url.pathname.split('/').filter(Boolean);
    requests.push({ method: request.method, path: url.pathname, size, graph, authorization: request.headers.authorization });
    if (remote.delayNextMs) {
      const delay = remote.delayNextMs;
      remote.delayNextMs = 0;
      await new Promise((r) => setTimeout(r, delay));
    }
    if (prefix !== 'c' || !SIZES.includes(size) || !graph) {
      response.writeHead(404, { ...cors, 'content-type': 'application/json' });
      return response.end(JSON.stringify({ ok: false, error: 'not found' }));
    }
    const headers = {};
    for (const name of ['authorization', 'content-type']) if (request.headers[name]) headers[name] = request.headers[name];
    const upstream = await fetch(`${zega.url}/${rest.join('/')}${url.search}`, {
      method: request.method, headers, body: payload.length ? payload : undefined,
    });
    response.writeHead(upstream.status, { ...cors, 'content-type': upstream.headers.get('content-type') || 'application/json' });
    response.end(Buffer.from(await upstream.arrayBuffer()));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  remote.url = `http://127.0.0.1:${server.address().port}`;
  remote.stop = async () => {
    server.closeAllConnections();
    await new Promise((r) => server.close(r));
    await zega.stop();
    await rm(directory, { recursive: true, force: true });
  };
  return remote;
}
