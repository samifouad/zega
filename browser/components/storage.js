// Where the canvas keeps its history (APS 21 §3). Every store holds the same
// plain-JSON shapes, so a notebook moves between the browser, an account's
// encrypted sync and the desktop's file system unchanged:
//
//   entry  { id, n, parent, time, spec, resultRef, meta }
//          id: a random UUID (unique across tabs and devices); n: an ordinal for
//          display; parent: the id of the entry it was made from, or null;
//          resultRef: the SHA-256 of the result's JSON.
//   result stored once per resultRef, however many entries point at it.
//
// A store is:
//   list(): Promise<Entry[]>                  every entry, oldest first
//   getResult(ref): Promise<result | null>
//   put(entry, result): Promise<boolean>      the result (if new), then the entry; false if not saved
//   remove(ids): Promise<void>                drop entries, then results no entry points at
//   clear(): Promise<void>
//   watch?(listener): () => void              entries put or removed elsewhere (another tab)
// Entries and results are separate keys with no shared index, so two tabs
// writing at once cannot overwrite each other's entries.

/** A store that keeps nothing beyond the page: the default. */
export function memoryStore() {
  const entries = new Map(), results = new Map();
  const gc = () => {
    const used = new Set([...entries.values()].map((e) => e.resultRef));
    for (const ref of results.keys()) if (!used.has(ref)) results.delete(ref);
  };
  return {
    async list() {
      return entries(true).sort((a, b) => (typeof a.time === 'number' && typeof b.time === 'number' ? byTime(a, b) : 0));
    },
    async getResult(ref) { return results.has(ref) ? structuredClone(results.get(ref)) : null; },
    async put(entry, result) {
      if (!results.has(entry.resultRef)) results.set(entry.resultRef, structuredClone(result));
      entries.set(entry.id, structuredClone(entry));
      return true;
    },
    async remove(ids) { for (const id of ids) entries.delete(id); gc(); },
    async clear() { entries.clear(); results.clear(); },
  };
}

const byTime = (a, b) => a.time - b.time || a.n - b.n || (a.id < b.id ? -1 : 1);

/**
 * The browser's localStorage under `key`: `<key>:e:<id>` per entry and
 * `<key>:r:<resultRef>` per result. A full or unavailable localStorage
 * (private mode, quota) never breaks the canvas: the history carries on in
 * memory and the failure goes to `onError`. A result is written before its
 * entry and taken back if the entry's write fails.
 */
export function localStorageStore(key = 'zega.canvas.history', { onError = (error) => console.warn(`canvas history: ${error.message}`) } = {}) {
  const E = `${key}:e:`, R = `${key}:r:`;
  const attempt = (work, fallback) => { try { return work(); } catch (error) { onError(error); return fallback; } };
  const keys = (prefix) => attempt(() => {
    const out = [];
    for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); if (k?.startsWith(prefix)) out.push(k); }
    return out;
  }, []);
  const parse = (k) => { try { return JSON.parse(localStorage.getItem(k)); } catch { return null; } };
  const entries = (report) => {
    const out = [];
    for (const k of keys(E)) {
      const entry = parse(k);
      if (entry && typeof entry === 'object') out.push(entry);
      else if (report) onError(new Error(`unreadable entry ${k}`));
    }
    return out;
  };
  const store = {
    async list() {
      const out = [];
      for (const k of keys(E)) {
        const entry = parse(k);
        if (entry && typeof entry === 'object') out.push(entry);
        else if (entry === null) onError(new Error(`unreadable entry ${k}`));
      }
      return out.sort((a, b) => (typeof a.time === 'number' && typeof b.time === 'number' ? byTime(a, b) : 0));
    },
    async getResult(ref) { return typeof ref === 'string' ? parse(`${R}${ref}`) : null; },
    async put(entry, result) {
      const resultKey = `${R}${entry.resultRef}`;
      const had = attempt(() => localStorage.getItem(resultKey) !== null, false);
      if (!had && !attempt(() => { localStorage.setItem(resultKey, JSON.stringify(result)); return true; }, false)) return false;
      if (attempt(() => { localStorage.setItem(`${E}${entry.id}`, JSON.stringify(entry)); return true; }, false)) return true;
      // The entry did not fit: take back the result written for it, so nothing is orphaned.
      if (!had) attempt(() => localStorage.removeItem(resultKey));
      return false;
    },
    async remove(ids) {
      for (const id of ids) attempt(() => localStorage.removeItem(`${E}${id}`));
      await store.gc();
    },
    /** Remove results no entry points at. */
    async gc() {
      const used = new Set(entries(false).map((e) => e.resultRef));
      for (const k of keys(R)) if (!used.has(k.slice(R.length))) attempt(() => localStorage.removeItem(k));
    },
    async clear() {
      for (const k of [...keys(E), ...keys(R)]) attempt(() => localStorage.removeItem(k));
    },
    /** `listener({ type: 'put', entry } | { type: 'remove', id })` for changes made in another tab. */
    watch(listener) {
      const on = (event) => {
        if (event.storageArea !== localStorage || !event.key?.startsWith(E)) return;
        if (event.newValue === null) listener({ type: 'remove', id: event.key.slice(E.length) });
        else {
          let entry = null;
          try { entry = JSON.parse(event.newValue); } catch (error) { onError(error); }
          if (entry) listener({ type: 'put', entry });
        }
      };
      addEventListener('storage', on);
      return () => removeEventListener('storage', on);
    },
  };
  return store;
}
