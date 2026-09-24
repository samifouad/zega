// explorer2: the explorer against a remote zega server: one Fly Machine
// (zega-explorer2-api) that Fly's proxy stops when idle and starts on the next
// request, with the graph on a volume. `npm run build -- --remote`
// puts this file in dist-remote/ as backend.js; the regular build never
// includes it. The protocol is the native backend's (native.js is backend.js
// with its class exported); only the transport differs: an absolute URL, a
// Bearer token the viewer types in, and a latency readout on every request.
import { NativeDatabase } from './native.js';

const KEYS = { token: 'zega.remote.token' };
// A request slower than this is almost always the Machine waking up.
const WAKING_AFTER_MS = 1000;

const $ = (sel) => document.querySelector(sel);

function element(tag, props = {}, children = []) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

const ms = (value) => `${Math.round(value)} ms`;

export async function connectDatabase(parser) {
  const response = await fetch('./explorer-config.json');
  if (!response.ok) throw new Error(`Cannot configure explorer: HTTP ${response.status}`);
  const config = await response.json();
  if (config.backend !== 'remote' || !config.base) throw new Error('Unknown explorer backend');
  document.head.append(element('link', { rel: 'stylesheet', href: 'remote.css' }));
  const ui = new RemoteBar(config.base);
  if (!localStorage.getItem(KEYS.token)) await ui.askToken();
  const db = new RemoteDatabase(parser, config.base, ui);
  try {
    await db.refresh();
  } catch (error) {
    // A rejected token reloads the page once a new one is saved; until then
    // the explorer waits rather than starting against a server it cannot use.
    if (error instanceof TokenRejected) await new Promise(() => {});
    ui.status(error.message, 'warn');
    throw error;
  }
  return db;
}

class TokenRejected extends Error {}

class RemoteDatabase extends NativeDatabase {
  remote = true;
  constructor(parser, base, ui) {
    super(parser);
    this.base = base;
    this.ui = ui;
  }
  async request(path, method = 'GET', body) {
    const headers = { Authorization: `Bearer ${localStorage.getItem(KEYS.token)}` };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const timing = this.ui.begin(method, path);
    let response, result;
    try {
      response = await fetch(this.base + path, {
        method, headers, mode: 'cors', credentials: 'omit', cache: 'no-store',
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      result = await response.json().catch(() => ({}));
    } catch (error) {
      timing.fail();
      throw new Error(`cannot reach ${new URL(this.base).host}: ${error.message}`);
    }
    timing.end();
    if (response.status === 401) {
      this.ui.rejected();
      throw new TokenRejected('the access token was rejected');
    }
    if (!response.ok || !result.ok) throw new Error(result.error || `HTTP ${response.status}`);
    return result.result;
  }
  async refresh() {
    await super.refresh();
    this.ui.graphSize(this.snapshot.nodes.length);
  }
}

class RemoteBar {
  samples = [];
  pending = 0;
  constructor(base) {
    this.host = new URL(base).host;
    this.latency = element('span', { id: 'remote-latency', textContent: 'no requests yet' });
    this.statusText = element('span', { id: 'remote-status', role: 'status' });
    this.empty = element('span', { id: 'remote-empty', hidden: true, textContent: 'graph is empty' });
    const sample = element('button', { id: 'remote-sample', textContent: 'load sample', title: 'load the Calgary sample into this remote graph' });
    sample.onclick = () => $('#btn-calgary').click();
    const forget = element('button', { id: 'remote-forget', textContent: 'forget token' });
    forget.onclick = () => { localStorage.removeItem(KEYS.token); location.reload(); };
    this.bar = element('div', { id: 'remote-bar' }, [
      element('span', { className: 'remote-where' }, [
        element('span', { className: 'conn-dot' }), `remote · ${this.host} · Fly shared-cpu-1x, 256 MB`,
      ]),
      this.latency, this.statusText, element('span', { className: 'spacer' }), this.empty, sample, forget,
    ]);
    $('#topbar').after(this.bar);
  }

  status(text, kind = '') {
    this.statusText.textContent = text;
    this.statusText.dataset.kind = kind;
  }

  /** Times one request; the readout shows it and the running median. */
  begin(method, path) {
    const started = performance.now();
    this.pending++;
    const waking = setTimeout(() => this.status('waking the Machine (it stops when idle)…', 'waking'), WAKING_AFTER_MS);
    const settle = () => {
      clearTimeout(waking);
      this.pending--;
      if (!this.pending && this.statusText.dataset.kind === 'waking') this.status('');
    };
    return {
      end: () => {
        const elapsed = performance.now() - started;
        settle();
        this.samples.push(elapsed);
        const middle = median(this.samples);
        this.latency.textContent = `last ${ms(elapsed)} (${method} ${path.split('/').slice(0, 3).join('/')}) · median ${ms(middle)} over ${this.samples.length}`;
        this.latency.dataset.last = String(Math.round(elapsed));
        this.latency.dataset.median = String(Math.round(middle));
        this.latency.dataset.count = String(this.samples.length);
        if (elapsed > WAKING_AFTER_MS) this.status(`Machine answered after ${(elapsed / 1000).toFixed(1)} s (woke from idle stop)`, 'cold');
      },
      fail: settle,
    };
  }

  graphSize(nodes) { this.empty.hidden = nodes > 0; }

  /** Resolves once the viewer has saved a token. */
  askToken(message = '') {
    const input = element('input', { id: 'remote-token-input', type: 'password', autocomplete: 'off', required: true, spellcheck: false });
    const dialog = element('dialog', { id: 'remote-token' }, [element('form', { method: 'dialog' }, [
      element('h2', { textContent: 'Remote zega access token' }),
      element('p', { textContent: `explorer2 runs your queries on zega on a Fly Machine at ${this.host}. The token stays in this browser (localStorage) and is sent only there, as a Bearer header.` }),
      ...(message ? [element('p', { className: 'remote-error', textContent: message })] : []),
      element('label', { htmlFor: 'remote-token-input', textContent: 'access token' }), input,
      element('footer', {}, [element('button', { type: 'submit', className: 'primary', textContent: 'Save token' })]),
    ])]);
    dialog.oncancel = (event) => event.preventDefault();
    document.body.append(dialog);
    dialog.showModal();
    return new Promise((resolve) => {
      dialog.onclose = () => {
        localStorage.setItem(KEYS.token, input.value.trim());
        dialog.remove();
        resolve();
      };
    });
  }

  /** A 401: drop the token, ask again, then start over with the new one. */
  rejected() {
    localStorage.removeItem(KEYS.token);
    this.status('access token rejected', 'warn');
    if ($('#remote-token')) return;
    this.askToken('That token was rejected. Enter it again.').then(() => location.reload());
  }
}
