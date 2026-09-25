// The shared canvas (APS 21 §2): one surface instance that every view is
// flushed from and loaded into, and a notebook history of immutable entries.
//
//   const canvas = createCanvas(element, { theme: 'light', store: localStorageStore() });
//   await canvas.ready;                              // history read back from the store
//   const entry = await canvas.load(spec, result);   // validate, flush, draw, record
//   canvas.flush();                                  // clear the canvas; history stays
//   await canvas.restore(0);                         // redraw entry 0 from its snapshot, no re-query
//   await canvas.undo();                             // back to the current entry's parent
//   canvas.history;                                  // [{ id, parent, time, spec, result, meta }]
//   canvas.on((event) => …);                         // after every load / restore / flush
//
// A spec that fails validation throws a ViewSpecError (teaching errors) and
// leaves the canvas and history exactly as they were. The surface — and so
// the WebGL context — is created once, on the first load that needs it, and
// reused by every later load, restore and theme change. The gear shows the
// current component's advertised parameters (gear.js); each edit is a new
// entry, so it can be undone.
import { COMPONENTS, SURFACES } from './registry.js';
import { ViewSpecError, validate } from './validate.js';
import { createGear } from './gear.js';
import { memoryStore } from './storage.js';

function deepFreeze(value) {
  if (value && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const key of Object.keys(value)) deepFreeze(value[key]);
  }
  return value;
}

export function createCanvas(container, { theme = 'light', historyLimit = 100, store = memoryStore() } = {}) {
  const host = document.createElement('div');
  host.className = 'zc-canvas';
  container.append(host);
  const surfaces = new Map(); // kind -> Promise<surface>
  const history = [];
  const listeners = new Set();
  let current = -1;
  let nextId = 1;
  let queue = Promise.resolve();
  let lastSwapMs = null;
  let swaps = 0;

  const surfaceFor = (kind) => {
    if (!surfaces.has(kind)) surfaces.set(kind, SURFACES[kind]().then((module) => module.createSurface(host)));
    return surfaces.get(kind);
  };
  // Serialise every operation: a second load never interleaves with the first's drawing.
  const serial = (task) => {
    const run = queue.then(task);
    queue = run.catch(() => {});
    return run;
  };
  const emit = (type) => { for (const listener of listeners) listener({ type, current, entry: history[current] || null }); };

  async function draw(spec, data) {
    const component = COMPONENTS[spec.component];
    const [module, surface] = await Promise.all([component.load(), surfaceFor(component.contract.surface)]);
    for (const [kind, other] of surfaces) if (kind !== component.contract.surface) (await other).clear();
    surface.clear();
    await surface.prepare({ theme, basemap: spec.surface.basemap, bounds: module.extent ? module.extent(data) : null });
    module.mount(surface, { data, spec, theme });
    await surface.rendered();
    gear.show(component.contract, spec);
  }
  function checked(spec, result) {
    const outcome = validate(spec, result, COMPONENTS[spec?.component]?.contract);
    if (!outcome.ok) throw new ViewSpecError(outcome.errors);
    return outcome;
  }
  async function timed(work) {
    const started = performance.now();
    await work();
    lastSwapMs = performance.now() - started;
    swaps++;
  }
  async function record(entry) {
    history.push(entry);
    const dropped = history.length > historyLimit ? history.splice(0, history.length - historyLimit) : [];
    current = history.length - 1;
    await store.put(entry);
    if (dropped.length) await store.remove(dropped.map((e) => e.id));
  }
  function add(spec, result, meta) {
    const { spec: resolved, data } = checked(spec, result);
    const entry = deepFreeze({ id: nextId++, parent: current >= 0 ? history[current].id : null, time: Date.now(),
      spec: resolved, result: structuredClone(result), meta: structuredClone(meta) });
    return { entry, data };
  }

  // A gear edit is a new entry from the current one's snapshot, with one parameter changed.
  const gear = createGear(host, (section, key, value) => serial(async () => {
    const from = history[current];
    if (!from) return;
    let made;
    try {
      made = add({ ...from.spec, [section]: { ...from.spec[section], [key]: value } }, from.result, { ...from.meta, edit: { [`${section}.${key}`]: value } });
    } catch (error) {
      if (!(error instanceof ViewSpecError)) throw error;
      gear.error(error.message);
      return;
    }
    await timed(() => draw(made.entry.spec, made.data));
    await record(made.entry);
    emit('load');
  }));

  const canvas = {
    /** Resolves once the store's history has been read back. */
    ready: serial(async () => {
      const saved = (await store.list()).slice(-historyLimit);
      for (const entry of saved) history.push(deepFreeze(entry));
      nextId = Math.max(0, ...history.map((e) => e.id)) + 1;
    }),
    get history() { return history; },
    /** Index into `history` of what is on the canvas, or -1 when it is empty. */
    get current() { return current; },
    get theme() { return theme; },
    stats() { return { swaps, lastSwapMs, entries: history.length, surfaces: surfaces.size }; },
    /** The surface of `kind` if it exists (for tests and the console). */
    surface: (kind = 'map') => surfaces.get(kind),
    gear,
    /** `listener({ type, current, entry })` after every load, restore and flush. Returns an unsubscribe. */
    on(listener) { listeners.add(listener); return () => listeners.delete(listener); },

    /**
     * Validate `spec` against its contract and `result`, then flush and draw
     * it, and record `{ spec, result snapshot, meta }` as a new entry whose
     * parent is the entry that was on the canvas (so going back and loading
     * something else branches instead of erasing). Resolves with the entry.
     */
    load(spec, result, meta = {}) {
      return serial(async () => {
        const { entry, data } = add(spec, result, meta);
        await timed(() => draw(entry.spec, data));
        await record(entry);
        emit('load');
        return entry;
      });
    },
    /** Redraw history entry `index` exactly, from its snapshot: no query runs. */
    restore(index) {
      return serial(async () => {
        const entry = history[index];
        if (!entry) throw new RangeError(`No history entry ${index}; there are ${history.length}`);
        // Drawn with the contract as it is now: an option added since the entry
        // was made shows at its default.
        const { spec, data } = checked(entry.spec, entry.result);
        await timed(() => draw(spec, data));
        current = index;
        emit('restore');
        return entry;
      });
    },
    /** Restore the current entry's parent (the view before an edit), if it is still in the history. */
    async undo() {
      await queue;
      const parent = history[current]?.parent;
      const index = history.findIndex((e) => e.id === parent);
      return index < 0 ? null : canvas.restore(index);
    },
    /** Clear what is drawn. The surface and the history stay. */
    flush() {
      return serial(async () => {
        for (const surface of surfaces.values()) (await surface).clear();
        gear.hide();
        current = -1;
        emit('flush');
      });
    },
    /** Empty the history, here and in the store. */
    clearHistory() {
      return serial(async () => {
        history.length = 0;
        current = -1;
        await store.clear();
        for (const surface of surfaces.values()) (await surface).clear();
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
        const { spec, data } = checked(entry.spec, entry.result);
        await draw(spec, data);
      });
    },
    /** Wait until the surface's tiles have loaded and the camera is still. */
    async idle() {
      await queue;
      for (const surface of surfaces.values()) await (await surface).idle();
    },
    async destroy() {
      await queue;
      for (const surface of surfaces.values()) (await surface).destroy();
      surfaces.clear();
      listeners.clear();
      host.remove();
    },
  };
  return canvas;
}
