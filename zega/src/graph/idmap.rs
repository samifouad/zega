//! Records by id, stored densely (zegadb/zega#100).
//!
//! Ids come from counters, so they are dense: a slot array indexed by id
//! costs the record and nothing else, where a `HashMap` adds the key, a
//! control byte and up to half its table empty (a million entries sit in a
//! table of two million). Ids an import or a restore brings may have gaps,
//! so an id far past the dense range goes to a small `HashMap` instead of
//! stretching the array: the array only grows while it stays at least half
//! full, and a hostile id like `2^40` costs one map entry.
//!
//! The slots live in fixed chunks, so growing never copies the whole array
//! and never leaves more than one chunk unused at the end.

use std::collections::HashMap;

const CHUNK_BITS: u32 = 10;
const CHUNK: usize = 1 << CHUNK_BITS;

pub(crate) struct IdMap<T> {
    chunks: Vec<Box<[Option<T>]>>,
    /// Ids at or past the dense range when they were inserted. Every key
    /// here is at least `self.dense_end()`.
    sparse: HashMap<u64, T>,
    len: usize,
}

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        IdMap {
            chunks: Vec::new(),
            sparse: HashMap::new(),
            len: 0,
        }
    }
}

fn split(id: u64) -> (usize, usize) {
    ((id >> CHUNK_BITS) as usize, (id as usize) & (CHUNK - 1))
}

impl<T> IdMap<T> {
    /// The first id past the dense range.
    fn dense_end(&self) -> u64 {
        (self.chunks.len() as u64) << CHUNK_BITS
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, id: u64) -> Option<&T> {
        if id < self.dense_end() {
            let (chunk, slot) = split(id);
            self.chunks[chunk][slot].as_ref()
        } else {
            self.sparse.get(&id)
        }
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut T> {
        if id < self.dense_end() {
            let (chunk, slot) = split(id);
            self.chunks[chunk][slot].as_mut()
        } else {
            self.sparse.get_mut(&id)
        }
    }

    /// Store `value` at `id`, returning what was there.
    pub fn insert(&mut self, id: u64, value: T) -> Option<T> {
        self.grow_to(id);
        let previous = if id < self.dense_end() {
            let (chunk, slot) = split(id);
            self.chunks[chunk][slot].replace(value)
        } else {
            self.sparse.insert(id, value)
        };
        if previous.is_none() {
            self.len += 1;
        }
        previous
    }

    pub fn remove(&mut self, id: u64) -> Option<T> {
        let removed = if id < self.dense_end() {
            let (chunk, slot) = split(id);
            self.chunks[chunk][slot].take()
        } else {
            self.sparse.remove(&id)
        };
        if removed.is_some() {
            self.len -= 1;
        }
        removed
    }

    /// The value at `id`, inserting `T::default()` first if there is none.
    pub fn get_or_default(&mut self, id: u64) -> &mut T
    where
        T: Default,
    {
        if self.get(id).is_none() {
            self.insert(id, T::default());
        }
        self.get_mut(id).expect("inserted just now")
    }

    /// Extend the dense range to cover `id` if that keeps the slots at least
    /// half used (plus one chunk of slack), and move any sparse entries it
    /// now covers into their slots.
    fn grow_to(&mut self, id: u64) {
        if id < self.dense_end() {
            return;
        }
        // In u64: on wasm32 a far id's chunk number does not fit a usize.
        let chunks = (id >> CHUNK_BITS) + 1;
        let allowed = (self.len as u64 + 1).saturating_mul(2) + CHUNK as u64;
        if chunks.saturating_mul(CHUNK as u64) > allowed {
            return;
        }
        while (self.chunks.len() as u64) < chunks {
            self.chunks.push((0..CHUNK).map(|_| None).collect());
        }
        if !self.sparse.is_empty() {
            let end = self.dense_end();
            let covered: Vec<u64> = self.sparse.keys().copied().filter(|&id| id < end).collect();
            for id in covered {
                let value = self.sparse.remove(&id).expect("listed just now");
                let (chunk, slot) = split(id);
                self.chunks[chunk][slot] = Some(value);
            }
        }
    }

    /// Every entry, ascending by id.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &T)> + '_ {
        let dense = self.chunks.iter().enumerate().flat_map(|(chunk, slots)| {
            let base = (chunk as u64) << CHUNK_BITS;
            slots
                .iter()
                .enumerate()
                .filter_map(move |(slot, value)| Some((base + slot as u64, value.as_ref()?)))
        });
        let mut sparse: Vec<(u64, &T)> = self.sparse.iter().map(|(id, value)| (*id, value)).collect();
        sparse.sort_unstable_by_key(|(id, _)| *id);
        dense.chain(sparse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The same operations on an `IdMap` and a `BTreeMap`, with dense ids,
    /// gaps, far ids that later become dense, and removals.
    #[test]
    fn behaves_as_a_map_by_id() {
        let mut map = IdMap::default();
        let mut model = BTreeMap::new();
        let mut state = 0x1d_u64;
        let mut next = 0u64;
        for step in 0..30_000u64 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let roll = (state >> 40) % 100;
            let id = match roll {
                0..=59 => {
                    next += 1;
                    next
                }
                60..=64 => next + 3_000 + (state >> 20) % 5_000,
                65 => 1 << 40,
                _ => (state >> 16) % (next + 1),
            };
            if roll < 85 {
                assert_eq!(map.insert(id, step), model.insert(id, step), "insert {id}");
            } else {
                assert_eq!(map.remove(id), model.remove(&id), "remove {id}");
            }
            assert_eq!(map.len(), model.len());
            assert_eq!(map.get(id), model.get(&id));
        }
        let got: Vec<(u64, u64)> = map.iter().map(|(id, v)| (id, *v)).collect();
        let want: Vec<(u64, u64)> = model.iter().map(|(id, v)| (*id, *v)).collect();
        assert_eq!(got, want);
        assert!(!map.sparse.is_empty(), "the far ids stayed sparse");
    }

    #[test]
    fn far_ids_do_not_stretch_the_slots() {
        let mut map = IdMap::default();
        map.insert(1 << 40, "far");
        map.insert(u64::MAX, "last");
        assert!(map.chunks.is_empty());
        for id in 0..5_000 {
            map.insert(id, "near");
        }
        assert_eq!(map.chunks.len(), 5_000usize.div_ceil(CHUNK));
        assert_eq!(map.get(1 << 40), Some(&"far"));
        assert_eq!(map.get(u64::MAX), Some(&"last"));
        assert_eq!(map.iter().last(), Some((u64::MAX, &"last")));
    }

    #[test]
    fn ids_inserted_before_the_range_reached_them_move_into_slots() {
        let mut map = IdMap::default();
        map.insert(1_500, 'b');
        assert!(map.chunks.len() <= 2);
        for id in 0..3_000 {
            if id != 1_500 {
                map.insert(id, 'a');
            }
        }
        assert!(map.sparse.is_empty());
        assert_eq!(map.get(1_500), Some(&'b'));
        assert_eq!(map.len(), 3_000);
    }
}
