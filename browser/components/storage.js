// Where the canvas keeps its history (APS 21 §3). Every store holds the same
// entry shape — `{ id, parent, time, spec, result, meta }`, plain JSON — so a
// notebook moves between the browser, an account's encrypted sync and the
// desktop's file system unchanged. Only the browser store exists today.
//
// A store is:
//   list(): Promise<Entry[]>      every entry, oldest first
//   put(entry): Promise<void>     add (or replace) one entry
//   remove(ids): Promise<void>    drop entries by id (history trimmed to its limit)
//   clear(): Promise<void>
// Entries are appended and dropped one at a time rather than the history being
// rewritten, so a syncing store can send only what changed.

/** A store that keeps nothing beyond the page: the default. */
export function memoryStore() {
  let entries = [];
  return {
    async list() { return entries.slice(); },
    async put(entry) { entries = [...entries.filter((e) => e.id !== entry.id), entry]; },
    async remove(ids) { const drop = new Set(ids); entries = entries.filter((e) => !drop.has(e.id)); },
    async clear() { entries = []; },
  };
}

/**
 * The browser's localStorage: one key per entry (`<key>:<id>`) and an index
 * of ids (`<key>`), so adding an entry writes only that entry. A full or
 * unavailable localStorage (private mode, quota) never breaks the canvas: the
 * history carries on in memory and the failure goes to `onError`.
 */
export function localStorageStore(key = 'zega.canvas.history', { onError = (error) => console.warn(`canvas history not saved: ${error.message}`) } = {}) {
  const attempt = (work, fallback) => { try { return work(); } catch (error) { onError(error); return fallback; } };
  const ids = () => attempt(() => { const list = JSON.parse(localStorage.getItem(key) || '[]'); return Array.isArray(list) ? list : []; }, []);
  const setIds = (list) => attempt(() => localStorage.setItem(key, JSON.stringify(list)));
  return {
    async list() {
      return ids().map((id) => attempt(() => JSON.parse(localStorage.getItem(`${key}:${id}`)), null)).filter(Boolean);
    },
    async put(entry) {
      if (attempt(() => { localStorage.setItem(`${key}:${entry.id}`, JSON.stringify(entry)); return true; }, false)) {
        const list = ids();
        if (!list.includes(entry.id)) setIds([...list, entry.id]);
      }
    },
    async remove(drop) {
      const gone = new Set(drop);
      for (const id of gone) attempt(() => localStorage.removeItem(`${key}:${id}`));
      setIds(ids().filter((id) => !gone.has(id)));
    },
    async clear() {
      for (const id of ids()) attempt(() => localStorage.removeItem(`${key}:${id}`));
      attempt(() => localStorage.removeItem(key));
    },
  };
}
