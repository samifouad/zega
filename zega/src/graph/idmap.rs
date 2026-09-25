//! Records by id, in slots (zegadb/zega#100).
//!
//! Ids come from counters, so they are mostly dense: a slot array indexed by
//! id costs the record and nothing else, where a `HashMap` adds the key, a
//! control byte and up to half its table empty (a million entries sit in a
//! table of two million).
//!
//! The slots come in chunks of [`CHUNK`] ids, allocated only where there are
//! entries. A chunk is allocated only while the allocated slots stay at most
//! twice the entries plus one chunk, and only within a directory span of
//! [`SPAN`] chunks per allocated chunk. An id that does not qualify waits in
//! an ordered overflow map, so a hostile id like `2^40` costs one map entry,
//! and scattered ids cost what a map costs. A chunk whose last entry goes is
//! freed and the directory trimmed, so a graph whose ids climb while old ones
//! are deleted (churn) keeps only the chunks that still hold entries. As the
//! entry count grows, overflow entries within reach move into chunks, so ids
//! that arrived out of order (a restore listing them descending) end up in
//! slots.
//!
//! What slots cannot give back: a chunk with some entries left keeps all its
//! slots, so deleting most of the ids at random leaves chunks partly empty.
//! A `HashMap` keeps its whole table after deletes too.

use std::collections::{BTreeMap, VecDeque};

const CHUNK_BITS: u32 = 10;
const CHUNK: usize = 1 << CHUNK_BITS;
/// Directory entries allowed per allocated chunk (plus the same again), so
/// the directory costs a few percent of the slots at most.
const SPAN: u64 = 64;

struct Chunk<T> {
    live: usize,
    slots: Box<[Option<T>]>,
}

pub(crate) struct IdMap<T> {
    /// The chunk number of `chunks[0]`.
    base: u64,
    /// `None` where a chunk is not allocated; never `None` at either end.
    chunks: VecDeque<Option<Chunk<T>>>,
    /// Every entry whose chunk is not allocated.
    sparse: BTreeMap<u64, T>,
    len: usize,
    allocated: usize,
    /// The overflow map's size after the last [`IdMap::rebuild`].
    settled: usize,
}

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        IdMap {
            base: 0,
            chunks: VecDeque::new(),
            sparse: BTreeMap::new(),
            len: 0,
            allocated: 0,
            settled: 0,
        }
    }
}

fn split(id: u64) -> (u64, usize) {
    (id >> CHUNK_BITS, (id as usize) & (CHUNK - 1))
}

impl<T> IdMap<T> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn chunk(&self, number: u64) -> Option<&Chunk<T>> {
        let at = number.checked_sub(self.base)?;
        self.chunks.get(usize::try_from(at).ok()?)?.as_ref()
    }

    fn chunk_mut(&mut self, number: u64) -> Option<&mut Chunk<T>> {
        let at = number.checked_sub(self.base)?;
        self.chunks.get_mut(usize::try_from(at).ok()?)?.as_mut()
    }

    pub fn get(&self, id: u64) -> Option<&T> {
        let (number, slot) = split(id);
        match self.chunk(number) {
            Some(chunk) => chunk.slots[slot].as_ref(),
            None => self.sparse.get(&id),
        }
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut T> {
        let (number, slot) = split(id);
        if self.chunk(number).is_some() {
            return self.chunk_mut(number).and_then(|chunk| chunk.slots[slot].as_mut());
        }
        self.sparse.get_mut(&id)
    }

    /// Store `value` at `id`, returning what was there.
    pub fn insert(&mut self, id: u64, value: T) -> Option<T> {
        let (number, slot) = split(id);
        if self.chunk(number).is_none() && self.may_allocate(number) {
            self.allocate(number);
        }
        let previous = match self.chunk_mut(number) {
            Some(chunk) => {
                let previous = chunk.slots[slot].replace(value);
                if previous.is_none() {
                    chunk.live += 1;
                }
                previous
            }
            None => self.sparse.insert(id, value),
        };
        if previous.is_none() {
            self.len += 1;
            // Every chunk's worth of entries raises the allowance by two
            // chunks: take in the overflow entries that now fit.
            if self.len.is_multiple_of(CHUNK) && !self.sparse.is_empty() {
                if self.sparse.len() > 2 * self.settled + CHUNK && self.sparse.len() * 2 > self.len {
                    self.rebuild();
                } else {
                    self.absorb();
                }
            }
        }
        previous
    }

    pub fn remove(&mut self, id: u64) -> Option<T> {
        let (number, slot) = split(id);
        let Some(chunk) = self.chunk_mut(number) else {
            let removed = self.sparse.remove(&id);
            if removed.is_some() {
                self.len -= 1;
            }
            return removed;
        };
        let removed = chunk.slots[slot].take();
        if removed.is_some() {
            chunk.live -= 1;
            let empty = chunk.live == 0;
            self.len -= 1;
            if empty {
                self.free(number);
            }
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

    /// Whether chunk `number` may be allocated: within the slot allowance
    /// and the directory span.
    fn may_allocate(&self, number: u64) -> bool {
        let slots = (self.allocated as u64 + 1) * CHUNK as u64;
        if slots > (self.len as u64 + 1).saturating_mul(2) + CHUNK as u64 {
            return false;
        }
        let (low, high) = if self.chunks.is_empty() {
            (number, number)
        } else {
            let end = self.base + self.chunks.len() as u64 - 1;
            (self.base.min(number), end.max(number))
        };
        high - low < SPAN * (self.allocated as u64 + 1)
    }

    /// Allocate chunk `number` and move the overflow entries it covers in.
    fn allocate(&mut self, number: u64) {
        if self.chunks.is_empty() {
            self.base = number;
        }
        while number < self.base {
            self.chunks.push_front(None);
            self.base -= 1;
        }
        while number >= self.base + self.chunks.len() as u64 {
            self.chunks.push_back(None);
        }
        let mut chunk = Chunk {
            live: 0,
            slots: (0..CHUNK).map(|_| None).collect(),
        };
        let start = number << CHUNK_BITS;
        let covered: Vec<u64> = self.sparse.range(start..start + CHUNK as u64).map(|(id, _)| *id).collect();
        for id in covered {
            let value = self.sparse.remove(&id).expect("listed just now");
            chunk.slots[split(id).1] = Some(value);
            chunk.live += 1;
        }
        self.chunks[(number - self.base) as usize] = Some(chunk);
        self.allocated += 1;
    }

    /// Free empty chunk `number` and trim the directory's empty ends.
    fn free(&mut self, number: u64) {
        self.chunks[(number - self.base) as usize] = None;
        self.allocated -= 1;
        while matches!(self.chunks.front(), Some(None)) {
            self.chunks.pop_front();
            self.base += 1;
        }
        while matches!(self.chunks.back(), Some(None)) {
            self.chunks.pop_back();
        }
    }

    /// Move overflow entries into chunks while the allowance lasts, lowest
    /// ids first, looking only within the span a chunk could be allocated in.
    fn absorb(&mut self) {
        let reach = SPAN * (self.allocated as u64 + 2);
        let (low, high) = if self.chunks.is_empty() {
            let first = self.sparse.keys().next().map_or(0, |id| split(*id).0);
            (first, first.saturating_add(reach))
        } else {
            (self.base.saturating_sub(reach), (self.base + self.chunks.len() as u64).saturating_add(reach))
        };
        let mut next = low.checked_shl(CHUNK_BITS).filter(|s| s >> CHUNK_BITS == low).unwrap_or(0);
        let limit = high.checked_shl(CHUNK_BITS).filter(|s| s >> CHUNK_BITS == high).unwrap_or(u64::MAX);
        while let Some((&id, _)) = self.sparse.range(next..limit).next() {
            let number = split(id).0;
            if self.may_allocate(number) {
                self.allocate(number);
            } else if (self.allocated as u64 + 1) * CHUNK as u64
                > (self.len as u64 + 1).saturating_mul(2) + CHUNK as u64
            {
                return;
            }
            match (number + 1).checked_shl(CHUNK_BITS).filter(|s| s >> CHUNK_BITS == number + 1) {
                Some(after) => next = after,
                None => return,
            }
        }
    }

    /// Choose the chunks again from scratch, densest first. Most entries
    /// are in the overflow map, so the allocated chunks are in the wrong
    /// place: the first ids seen were far from where the graph lives (a far
    /// id before the dense ones). Runs only when the overflow map has at
    /// least doubled since the last time, so its cost amortises.
    fn rebuild(&mut self) {
        let base = self.base;
        for (at, chunk) in std::mem::take(&mut self.chunks).into_iter().enumerate() {
            let Some(chunk) = chunk else { continue };
            let start = (base + at as u64) << CHUNK_BITS;
            for (slot, value) in chunk.slots.into_vec().into_iter().enumerate() {
                if let Some(value) = value {
                    self.sparse.insert(start + slot as u64, value);
                }
            }
        }
        self.allocated = 0;
        self.base = 0;
        let mut counts: Vec<(u64, usize)> = Vec::new();
        for id in self.sparse.keys() {
            let number = split(*id).0;
            match counts.last_mut() {
                Some((last, count)) if *last == number => *count += 1,
                _ => counts.push((number, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (number, _) in counts {
            if self.may_allocate(number) {
                self.allocate(number);
            }
        }
        self.settled = self.sparse.len();
    }

    /// Every entry, ascending by id.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &T)> + '_ {
        let base = self.base;
        let mut dense = self
            .chunks
            .iter()
            .enumerate()
            .filter_map(move |(at, chunk)| Some((base + at as u64, chunk.as_ref()?)))
            .flat_map(|(number, chunk)| {
                let start = number << CHUNK_BITS;
                chunk
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(move |(slot, value)| Some((start + slot as u64, value.as_ref()?)))
            })
            .peekable();
        let mut sparse = self.sparse.iter().map(|(id, value)| (*id, value)).peekable();
        std::iter::from_fn(move || match (dense.peek(), sparse.peek()) {
            (Some(d), Some(s)) if s.0 < d.0 => sparse.next(),
            (Some(_), _) => dense.next(),
            (None, _) => sparse.next(),
        })
    }

    #[cfg(test)]
    fn allocated_chunks(&self) -> usize {
        self.allocated
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
        assert_eq!(map.allocated_chunks(), 1, "the first id gets a chunk");
        for id in 0..5_000 {
            map.insert(id, "near");
        }
        assert!(map.allocated_chunks() <= 5_000usize.div_ceil(CHUNK));
        assert_eq!(map.sparse.len(), 2, "the far ids moved to the overflow map");
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
        assert_eq!(map.allocated_chunks(), 50_000usize.div_ceil(CHUNK));
        assert!(map.iter().map(|(id, v)| (id, *v)).eq((0..50_000).map(|id| (id, id))));
    }

    /// Review M1 of #113: ids climb while old ones are deleted. The chunks
    /// the deletes empty are freed, so what stays allocated follows the
    /// live entries, not every id ever used.
    #[test]
    fn churn_frees_the_chunks_it_empties() {
        let mut map = IdMap::default();
        let window = 10_000u64;
        for id in 0..window {
            map.insert(id, id);
        }
        for id in window..window * 50 {
            map.insert(id, id);
            assert_eq!(map.remove(id - window), Some(id - window));
        }
        assert_eq!(map.len(), window as usize);
        assert!(map.allocated_chunks() <= window as usize / CHUNK + 2, "{}", map.allocated_chunks());
        assert!(map.chunks.len() <= window as usize / CHUNK + 2);
        assert!(map.sparse.is_empty());
        assert!(map.iter().map(|(id, v)| (id, *v)).eq((window * 49..window * 50).map(|id| (id, id))));
    }

    /// Ids one chunk apart would each take a chunk for one entry; past the
    /// allowance they stay in the overflow map instead.
    #[test]
    fn scattered_ids_do_not_each_take_a_chunk() {
        let mut map = IdMap::default();
        for k in 0..1_000u64 {
            map.insert(k * CHUNK as u64, k);
        }
        assert!(map.allocated_chunks() <= 2);
        assert_eq!(map.len(), 1_000);
        assert!(map.iter().map(|(id, v)| (id, *v)).eq((0..1_000).map(|k| (k * CHUNK as u64, k))));
    }

    #[test]
    fn ids_inserted_before_the_range_reached_them_move_into_slots() {
        let mut map = IdMap::default();
        map.insert(1_500, 'b');
        assert!(map.allocated_chunks() <= 2);
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
