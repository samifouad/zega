// The shared canvas (APS 21 §2): one surface instance that every view is
// flushed from and loaded into, and a notebook history of immutable entries.
//
//   const canvas = createCanvas(element, { theme: 'light', store: localStorageStore() });
//   await canvas.ready;                              // history read back from the store, checked
//   const entry = await canvas.load(spec, result);   // validate, draw, record
//   await canvas.flush();                            // clear the canvas; history stays
//   await canvas.restore(0);                         // redraw entry 0 from its snapshot, no re-query
//   await canvas.undo();                             // back to the current entry's parent
//   canvas.history;                                  // [{ id, n, parent, time, spec, resultRef, meta }]
//   canvas.resultOf(entry);                          // its result snapshot (frozen, shared by ref)
//   canvas.on((event) => …);                         // after every load / restore / flush / sync
//
// A spec that fails validation throws a ViewSpecError (teaching errors) and
// leaves the canvas and history exactly as they were; so does a draw that
// fails (the previous entry is drawn again). The surface — and so the WebGL
// context — is created once, on the first load that needs it, and reused by
// every later load, restore and theme change. Consecutive views of the same
// component update its layers in place rather than rebuilding them. The gear
// shows the current component's advertised parameters (gear.js); each edit is
// a new entry, so it can be undone.
import { SURFACES, component, surfaceRules } from './registry.js';
import { ViewSpecError, validate } from './validate.js';
import { createGear } from './gear.js';
import { memoryStore } from './storage.js';

const MAX_DEPTH = 64;
/** A frozen deep copy of plain JSON, iteratively, skipping `__proto__` keys; a RangeError past MAX_DEPTH. */
export function frozenCopy(value) {
  if (value === null || typeof value !== 'object') return value;
  const root = Array.isArray(value) ? new Array(value.length) : {};
  const stack = [[value, root, 0]];
  const made = [];
  while (stack.length) {
    const [from, to, depth] = stack.pop();
    if (depth > MAX_DEPTH) throw new RangeError(`nested deeper than ${MAX_DEPTH} levels`);
    made.push(to);
    for (const key of Object.keys(from)) {
      if (key === '__proto__') continue;
      const v = from[key];
      if (v !== null && typeof v === 'object') {
        const copy = Array.isArray(v) ? new Array(v.length) : {};
        to[key] = copy;
        stack.push([v, copy, depth + 1]);
      } else to[key] = v;
    }
  }
  for (const object of made) Object.freeze(object);
  return root;
}

/** SHA-256 of the result's JSON: its key in the store. */
async function hashOf(json) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(json));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}
const newId = () => crypto.randomUUID();
const isEntry = (e) => e !== null && typeof e === 'object' && typeof e.id === 'string' && e.id.length <= 64
  && Number.isInteger(e.n) && e.n > 0 && (e.parent === null || typeof e.parent === 'string') && Number.isFinite(e.time)
  && e.spec !== null && typeof e.spec === 'object' && typeof e.resultRef === 'string' && /^[0-9a-f]{64}$/.test(e.resultRef)
  && e.meta !== null && typeof e.meta === 'object' && !Array.isArray(e.meta);

export function createCanvas(container, { theme = 'light', historyLimit = 100, store = memoryStore(), onError = (error) => console.warn(error.message) } = {}) {
  const host = document.createElement('div');
  host.className = 'zc-canvas';
  container.append(host);
  const surfaces = new Map(); // kind -> Promise<surface>
  const history = [];
  const results = new Map(); // resultRef -> frozen result
  const listeners = new Set();
  let current = -1;
  let ordinal = 0;
  let drawn = null; // { component, kind, handle } of what is on the surface
  let queue = Promise.resolve();
  let lastSwapMs = null;
  let swaps = 0;

  const surfaceFor = (kind) => {
    if (!surfaces.has(kind)) {
      const made = SURFACES[kind].load().then((module) => module.createSurface(host));
      made.catch(() => surfaces.delete(kind));
      surfaces.set(kind, made);
    }
    return surfaces.get(kind);
  };
  // Serialise every operation: a second load never interleaves with the first's drawing.
  const serial = (task) => {
    const run = queue.then(task);
    queue = run.catch(() => {});
    return run;
  };
  const emit = (type) => { for (const listener of listeners) listener({ type, current, entry: history[current] || null }); };
  const checked = (spec, result) => {
    const contract = component(spec?.component)?.contract;
    const outcome = validate(spec, result, contract, surfaceRules(contract));
    if (!outcome.ok) throw new ViewSpecError(outcome.errors);
    return outcome;
  };
  const pruneResults = () => {
    const used = new Set(history.map((e) => e.resultRef));
    for (const ref of results.keys()) if (!used.has(ref)) results.delete(ref);
  };

  // One swap may block for at most this long in all, including putting the previous view back.
  const SWAP_BUDGET = 5000;
  async function draw(spec, data, deadline = performance.now() + SWAP_BUDGET) {
    const entry = component(spec.component);
    const kind = entry.contract.surface;
    const [module, surface] = await Promise.all([entry.load(), surfaceFor(kind)]);
    for (const [other, made] of surfaces) if (other !== kind) (await made).clear();
    const restyled = await surface.prepare({ theme, basemap: spec.surface.basemap, bounds: module.extent ? module.extent(data) : null });
    const props = { data, spec, theme };
    if (!restyled && drawn && drawn.component === spec.component && drawn.kind === kind) drawn.handle.update(props);
    else {
      drawn = null;
      surface.clear();
      drawn = { component: spec.component, kind, handle: module.mount(surface, props) };
    }
    const left = deadline - performance.now();
    if (left > 0) await surface.rendered(left);
    gear.show(entry.contract, spec);
  }
  /** Draw; if that fails, put the previous entry (or nothing) back and rethrow. */
  async function show(spec, data) {
    const started = performance.now();
    const deadline = started + SWAP_BUDGET;
    try {
      await draw(spec, data, deadline);
    } catch (error) {
      drawn = null;
      for (const made of surfaces.values()) (await made.catch(() => null))?.clear();
      const previous = history[current];
      if (previous) {
        // Within what is left of the same budget; with none left it is mounted without waiting for a frame.
        try { await draw(previous.spec, checked(previous.spec, results.get(previous.resultRef)).data, deadline); }
        catch { drawn = null; current = -1; gear.hide(); }
      } else gear.hide();
      throw error;
    }
    lastSwapMs = performance.now() - started;
    swaps++;
  }
  async function add(spec, result, meta) {
    const { spec: resolved, data } = checked(spec, result);
    const json = JSON.stringify(result);
    const resultRef = await hashOf(json);
    if (!results.has(resultRef)) results.set(resultRef, frozenCopy(JSON.parse(json)));
    const entry = frozenCopy({ id: newId(), n: ++ordinal, parent: current >= 0 ? history[current].id : null, time: Date.now(),
      spec: resolved, resultRef, meta: JSON.parse(JSON.stringify(meta ?? {})) });
    return { entry, data };
  }
  async function record(entry) {
    history.push(entry);
    const dropped = history.length > historyLimit ? history.splice(0, history.length - historyLimit) : [];
    current = history.length - 1;
    if (!(await store.put(entry, results.get(entry.resultRef)))) onError(new Error(`entry #${entry.n} is not saved; it stays in this page's history`));
    if (dropped.length) await store.remove(dropped.map((e) => e.id));
    pruneResults();
  }
  async function commit(spec, result, meta) {
    const { entry, data } = await add(spec, result, meta);
    try { await show(entry.spec, data); } catch (error) { pruneResults(); throw error; }
    await record(entry);
    emit('load');
    return entry;
  }

  // A gear edit is a new entry from the current one's snapshot, with one parameter changed.
  const gear = createGear(host, (section, key, value) => serial(async () => {
    const from = history[current];
    if (!from) return;
    try {
      await commit({ ...from.spec, [section]: { ...from.spec[section], [key]: value } }, results.get(from.resultRef), { ...from.meta, edit: { [`${section}.${key}`]: value } });
    } catch (error) {
      gear.error(error instanceof ViewSpecError ? error.message : `not drawn: ${error.message}`);
    }
  }));

  /** Check a stored entry and fetch its result; null (and onError) when it cannot be used. */
  async function admit(entry) {
    if (!isEntry(entry)) { onError(new Error(`dropped a stored entry that is not an entry (${JSON.stringify(entry)?.slice(0, 80)})`)); return null; }
    let result = results.get(entry.resultRef);
    if (!result) {
      const stored = await store.getResult(entry.resultRef);
      if (stored === null || stored === undefined) { onError(new Error(`dropped entry #${entry.n}: its result is missing`)); return null; }
      const json = JSON.stringify(stored);
      if (await hashOf(json) !== entry.resultRef) { onError(new Error(`dropped entry #${entry.n}: its result does not match its hash`)); return null; }
      result = frozenCopy(stored);
    }
    try { checked(entry.spec, result); } catch (error) { onError(new Error(`dropped entry #${entry.n}: ${error.message.split('\n')[0]}`)); return null; }
    results.set(entry.resultRef, result);
    return frozenCopy(entry);
  }

  const canvas = {
    /** Resolves once the store's history has been read back and checked. */
    ready: serial(async () => {
      const stored = await store.list();
      const bad = [];
      for (const entry of stored) {
        const admitted = await admit(entry);
        if (admitted) history.push(admitted); else if (typeof entry?.id === 'string') bad.push(entry.id);
      }
      const excess = history.length > historyLimit ? history.splice(0, history.length - historyLimit).map((e) => e.id) : [];
      if (bad.length || excess.length) await store.remove([...bad, ...excess]);
      pruneResults();
      ordinal = Math.max(0, ...history.map((e) => e.n));
    }),
    get history() { return history; },
    /** Index into `history` of what is on the canvas, or -1 when it is empty. */
    get current() { return current; },
    get theme() { return theme; },
    /** The result snapshot an entry points at. */
    resultOf: (entry) => results.get(entry?.resultRef),
    stats() { return { swaps, lastSwapMs, entries: history.length, results: results.size, surfaces: surfaces.size }; },
    /** The surface of `kind` if it exists (for tests and the console). */
    surface: (kind = 'map') => surfaces.get(kind),
    gear,
    /** `listener({ type, current, entry })` after every load, restore, flush and sync. Returns an unsubscribe. */
    on(listener) { listeners.add(listener); return () => listeners.delete(listener); },

    /**
     * Validate `spec` against its contract and `result`, then draw it, and
     * record `{ spec, result snapshot, meta }` as a new entry whose parent is
     * the entry that was on the canvas (so going back and loading something
     * else branches instead of erasing). Resolves with the entry.
     */
    load(spec, result, meta = {}) { return serial(() => commit(spec, result, meta)); },
    /** Redraw history entry `index` exactly, from its snapshot: no query runs. */
    restore(index) {
      return serial(async () => {
        const entry = history[index];
        if (!entry) throw new RangeError(`No history entry ${index}; there are ${history.length}`);
        // Drawn with the contract as it is now: an option added since the entry was made shows at its default.
        const { spec, data } = checked(entry.spec, results.get(entry.resultRef));
        await show(spec, data);
        current = index;
        emit('restore');
        return entry;
      });
    },
    /** Restore the current entry's parent (the view before an edit), if it is still in the history. */
    undo() {
      return serial(async () => {
        const parent = history[current]?.parent;
        const index = parent ? history.findIndex((e) => e.id === parent) : -1;
        if (index < 0) return null;
        const entry = history[index];
        const { spec, data } = checked(entry.spec, results.get(entry.resultRef));
        await show(spec, data);
        current = index;
        emit('restore');
        return entry;
      });
    },
    /** Clear what is drawn. The surface and the history stay. */
    flush() {
      return serial(async () => {
        for (const surface of surfaces.values()) (await surface).clear();
        drawn = null;
        gear.hide();
        current = -1;
        emit('flush');
      });
    },
    /** Empty the history, here and in the store. */
    clearHistory() {
      return serial(async () => {
        history.length = 0;
        results.clear();
        current = -1;
        await store.clear();
        for (const surface of surfaces.values()) (await surface).clear();
        drawn = null;
        gear.hide();
        emit('flush');
      });
    },
    /** Switch light/dark and redraw the current entry in it. */
    setTheme(next) {
      return serial(async () => {
        theme = next;
        if (current < 0) return;
        const entry = history[current];
        const { spec, data } = checked(entry.spec, results.get(entry.resultRef));
        await show(spec, data);
      });
    },
    /** Wait until the surface's tiles have loaded and the camera is still. */
    async idle() {
      await queue;
      for (const surface of surfaces.values()) await (await surface).idle();
    },
    async destroy() {
      await queue;
      unwatch?.();
      for (const surface of surfaces.values()) (await surface).destroy();
      surfaces.clear();
      listeners.clear();
      host.remove();
    },
  };

  // Another tab's entries join this history (checked like stored ones); its removals leave it.
  const unwatch = store.watch?.((change) => serial(async () => {
    if (change.type === 'put') {
      if (history.some((e) => e.id === change.entry.id)) return;
      const admitted = await admit(change.entry);
      if (!admitted) return;
      const at = history.findIndex((e) => e.time > admitted.time);
      const index = at < 0 ? history.length : at;
      history.splice(index, 0, admitted);
      if (current >= index) current++;
      ordinal = Math.max(ordinal, admitted.n);
    } else {
      const index = history.findIndex((e) => e.id === change.id);
      if (index < 0) return;
      history.splice(index, 1);
      if (current === index) {
        // The entry on screen was trimmed or cleared in another tab: clear the view with it.
        current = -1;
        drawn = null;
        for (const made of surfaces.values()) (await made).clear();
        gear.hide();
      } else if (current > index) current--;
      pruneResults();
    }
    emit('sync');
  }));
  return canvas;
}
