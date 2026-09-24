//! A least-recently-used map bounded by bytes, not entries.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

pub struct ByteLru<K, V> {
    capacity: usize,
    used: usize,
    tick: u64,
    entries: HashMap<K, (V, usize, u64)>,
    order: BTreeMap<u64, K>,
}

impl<K: Clone + Eq + Hash, V: Clone> ByteLru<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, used: 0, tick: 0, entries: HashMap::new(), order: BTreeMap::new() }
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        let (value, _, last) = self.entries.get_mut(key)?;
        self.order.remove(last);
        self.tick += 1;
        *last = self.tick;
        self.order.insert(self.tick, key.clone());
        Some(value.clone())
    }

    pub fn put(&mut self, key: K, value: V, bytes: usize) {
        // Map + order bookkeeping per entry.
        let bytes = bytes + 96;
        if bytes > self.capacity {
            return;
        }
        self.remove(&key);
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.entries.insert(key, (value, bytes, self.tick));
        self.used += bytes;
        while self.used > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            if let Some((_, size, _)) = self.entries.remove(&oldest) {
                self.used -= size;
            }
        }
    }

    pub fn remove(&mut self, key: &K) {
        if let Some((_, size, last)) = self.entries.remove(key) {
            self.order.remove(&last);
            self.used -= size;
        }
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recent_within_budget() {
        let mut lru = ByteLru::new(3 * (100 + 96));
        lru.put(1, "a", 100);
        lru.put(2, "b", 100);
        lru.put(3, "c", 100);
        assert_eq!(lru.get(&1), Some("a"));
        lru.put(4, "d", 100);
        assert_eq!(lru.get(&2), None, "2 was least recently used");
        assert_eq!(lru.get(&1), Some("a"));
        assert!(lru.used() <= 3 * 196);
    }
}
