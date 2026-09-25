//! A set of node or relationship ids sized for how the indexes use it
//! (zegadb/zega#100).
//!
//! Most index entries hold one id (a unique value, a map cell with one
//! point) or a handful. A `HashSet` costs 48 bytes plus a table of at least
//! four slots for each of those; an [`IdSet`] is 24 bytes and holds one id
//! inline, up to [`FEW`] in a small vector, and switches to a hash set only
//! above that, so a set shared by many nodes still inserts and removes in
//! constant time.

use std::collections::HashSet;

/// The most ids kept in a plain vector; one more moves them to a hash set.
const FEW: usize = 16;

#[derive(Clone, Debug, Default)]
pub(crate) enum IdSet {
    #[default]
    Empty,
    One(u64),
    Few(Vec<u64>),
    Many(Box<HashSet<u64>>),
}

impl IdSet {
    /// Add `id`; false when it was already there.
    pub fn insert(&mut self, id: u64) -> bool {
        match self {
            IdSet::Empty => *self = IdSet::One(id),
            IdSet::One(one) => {
                if *one == id {
                    return false;
                }
                *self = IdSet::Few(vec![*one, id]);
            }
            IdSet::Few(ids) => {
                if ids.contains(&id) {
                    return false;
                }
                if ids.len() < FEW {
                    ids.push(id);
                } else {
                    let mut set: HashSet<u64> = ids.drain(..).collect();
                    set.insert(id);
                    *self = IdSet::Many(Box::new(set));
                }
            }
            IdSet::Many(set) => return set.insert(id),
        }
        true
    }

    /// Take `id` out; false when it was not there.
    pub fn remove(&mut self, id: u64) -> bool {
        match self {
            IdSet::Empty => false,
            IdSet::One(one) => {
                if *one != id {
                    return false;
                }
                *self = IdSet::Empty;
                true
            }
            IdSet::Few(ids) => {
                let Some(at) = ids.iter().position(|has| *has == id) else {
                    return false;
                };
                ids.swap_remove(at);
                if ids.len() == 1 {
                    *self = IdSet::One(ids[0]);
                }
                true
            }
            IdSet::Many(set) => {
                let removed = set.remove(&id);
                if set.len() <= FEW / 2 {
                    *self = IdSet::Few(set.iter().copied().collect());
                }
                removed
            }
        }
    }

    pub fn contains(&self, id: &u64) -> bool {
        match self {
            IdSet::Empty => false,
            IdSet::One(one) => one == id,
            IdSet::Few(ids) => ids.contains(id),
            IdSet::Many(set) => set.contains(id),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            IdSet::Empty => 0,
            IdSet::One(_) => 1,
            IdSet::Few(ids) => ids.len(),
            IdSet::Many(set) => set.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every id, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = &u64> {
        let (one, few, many) = match self {
            IdSet::Empty => (None, None, None),
            IdSet::One(one) => (Some(one), None, None),
            IdSet::Few(ids) => (None, Some(ids.iter()), None),
            IdSet::Many(set) => (None, None, Some(set.iter())),
        };
        one.into_iter()
            .chain(few.into_iter().flatten())
            .chain(many.into_iter().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same operations on an `IdSet` and a `HashSet`, through every
    /// representation and back.
    #[test]
    fn behaves_as_a_set_through_every_size() {
        let mut ids = IdSet::default();
        let mut model = HashSet::new();
        let mut state = 0x5eed_u64;
        for step in 0..20_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            // Grow for a while, then mostly shrink, so every size is visited.
            let id = (state >> 33) % 64;
            let grow = (step / 2000) % 2 == 0;
            let insert = (state >> 20) % 4 != 0;
            if insert == grow {
                assert_eq!(ids.insert(id), model.insert(id), "insert {id} at {step}");
            } else {
                assert_eq!(ids.remove(id), model.remove(&id), "remove {id} at {step}");
            }
            assert_eq!(ids.len(), model.len());
            assert_eq!(ids.is_empty(), model.is_empty());
            let mut got: Vec<u64> = ids.iter().copied().collect();
            got.sort_unstable();
            let mut want: Vec<u64> = model.iter().copied().collect();
            want.sort_unstable();
            assert_eq!(got, want, "at {step}");
            for probe in [id, id + 1, 70] {
                assert_eq!(ids.contains(&probe), model.contains(&probe));
            }
        }
    }

    #[test]
    fn small_sets_stay_inline_and_large_ones_hash() {
        let mut ids = IdSet::default();
        ids.insert(7);
        assert!(matches!(ids, IdSet::One(7)));
        for id in 0..=FEW as u64 {
            ids.insert(id);
        }
        assert!(matches!(ids, IdSet::Many(_)));
        for id in 0..=FEW as u64 {
            ids.remove(id);
        }
        assert!(ids.is_empty());
        assert_eq!(std::mem::size_of::<IdSet>(), 24);
    }
}
