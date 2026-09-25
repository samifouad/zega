//! Records by id, stored densely (zegadb/zega#100).
//!
//! Ids come from counters, so they are dense: a slot array indexed by id
//! costs the record and nothing else, where a `HashMap` adds the key, a
//! control byte and up to half its table empty (a million entries sit in a
//! table of two million). Ids an import or a restore brings may have gaps
//! or arrive in any order, so an id past the dense range waits in an
//! ordered overflow map instead of stretching the array: the array only
//! covers as many slots as twice the entries (plus one chunk), and a
//! hostile id like `2^40` costs one map entry. Whenever the array may grow,
//! it takes in the overflow entries it can now cover, so ids that arrive
//! out of order still end up in slots.
//!
//! The slots live in fixed chunks, so growing never copies the whole array
//! and never leaves more than one chunk unused at the end.

use std::collections::BTreeMap;

const CHUNK_BITS: u32 = 10;
const CHUNK: usize = 1 << CHUNK_BITS;

pub(crate) struct IdMap<T> {
    chunks: Vec<Box<[Option<T>]>>,
    /// Ids past the dense range. Every key here is at least
    /// `self.dense_end()`.
    sparse: BTreeMap<u64, T>,
    len: usize,
}

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        IdMap {
            chunks: Vec::new(),
            sparse: BTreeMap::new(),
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

    /// The most chunks the slots may span: twice the entries plus one chunk.
    /// In u64: on wasm32 a far id's chunk number does not fit a usize.
    fn allowed_chunks(&self) -> u64 {
        ((self.len as u64 + 1).saturating_mul(2) + CHUNK as u64) >> CHUNK_BITS
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
        if id >= self.dense_end() && (id >> CHUNK_BITS) < self.allowed_chunks() {
            self.grow((id >> CHUNK_BITS) + 1);
        }
        let previous = if id < self.dense_end() {
            let (chunk, slot) = split(id);
            self.chunks[chunk][slot].replace(value)
        } else {
            self.sparse.insert(id, value)
        };
        if previous.is_none() {
            self.len += 1;
            // Every chunk's worth of entries raises the allowance by two
            // chunks: take in the overflow entries that now fit.
            if self.len.is_multiple_of(CHUNK) && !self.sparse.is_empty() {
                self.grow(0);
            }
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

    /// Extend the slots to at least `chunks` chunks (within the allowance),
    /// and on while the next overflow entry fits, moving each entry the
    /// slots now cover into its slot.
    fn grow(&mut self, chunks: u64) {
        let allowed = self.allowed_chunks();
        let mut target = chunks.min(allowed);
        while let Some((&first, _)) = self.sparse.first_key_value() {
            let needed = (first >> CHUNK_BITS) + 1;
            if needed > allowed {
                break;
            }
            target = target.max(needed);
            while (self.chunks.len() as u64) < target {
                self.chunks.push((0..CHUNK).map(|_| None).collect());
            }
            let rest = self.sparse.split_off(&self.dense_end());
            for (id, value) in std::mem::replace(&mut self.sparse, rest) {
                let (chunk, slot) = split(id);
                self.chunks[chunk][slot] = Some(value);
            }
        }
        while (self.chunks.len() as u64) < target {
            self.chunks.push((0..CHUNK).map(|_| None).collect());
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
        dense.chain(self.sparse.iter().map(|(id, value)| (*id, value)))
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

    /// Descending ids go to the overflow map until the slots may reach
    /// them, then move into slots.
    #[test]
    fn ids_in_reverse_order_end_up_in_slots() {
        let mut map = IdMap::default();
        for id in (0..50_000u64).rev() {
            map.insert(id, id);
        }
        assert_eq!(map.len(), 50_000);
        assert!(map.sparse.is_empty());
        assert_eq!(map.chunks.len(), 50_000usize.div_ceil(CHUNK));
        assert!(map.iter().map(|(id, v)| (id, *v)).eq((0..50_000).map(|id| (id, id))));
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
