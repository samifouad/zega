//! `unique { Type { field } }` indexes (zegadb/zega#100).
//!
//! Only `unique` checks and lookups ever asked the graph for "the nodes
//! whose field is this value", so only the (type, field) pairs a schema
//! declares unique are indexed, not every property of every node. Like the
//! `index { }` declarations, the set follows the schema of the statement
//! being run: [`super::Graph::sync_uniques`] drops the others and builds a
//! new one from the nodes already stored, and every write keeps them
//! current after.
//!
//! An entry maps a per-graph keyed hash of the value to the ids holding it.
//! It stores no value: a lookup checks each candidate's stored value, so a
//! collision costs a comparison, never a wrong answer, and equality is
//! `Value`'s own (NaN payloads, `-0.0` and Int against Float exactly as a
//! map keyed by `Value` compared them).

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher, RandomState};

use super::names::{Shape, Sym};
use crate::idset::IdSet;
use crate::value::Value;

/// A hasher for keys that are already hashes.
#[derive(Default)]
struct PreHashed(u64);

impl Hasher for PreHashed {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 << 8) | u64::from(*byte);
        }
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

struct Slot {
    ty: Sym,
    field: Sym,
    ids: HashMap<u64, IdSet, BuildHasherDefault<PreHashed>>,
}

pub(crate) struct UniqueIndexes {
    /// Keys the value hash, per graph, so no one can choose values that
    /// collide.
    hasher: RandomState,
    slots: Vec<Slot>,
}

impl Default for UniqueIndexes {
    fn default() -> Self {
        UniqueIndexes {
            hasher: RandomState::new(),
            slots: Vec::new(),
        }
    }
}

impl UniqueIndexes {
    pub fn contains(&self, ty: Sym, field: Sym) -> bool {
        self.slots.iter().any(|slot| slot.ty == ty && slot.field == field)
    }

    /// Drop every index `keep` does not name.
    pub fn retain(&mut self, keep: &[(Sym, Sym)]) {
        self.slots.retain(|slot| keep.contains(&(slot.ty, slot.field)));
    }

    /// Start an empty index on (`ty`, `field`); the caller fills it.
    pub fn add(&mut self, ty: Sym, field: Sym) {
        if !self.contains(ty, field) {
            self.slots.push(Slot { ty, field, ids: HashMap::default() });
        }
    }

    /// Index node `id` (with `shape` and `values`) in every index whose type
    /// it carries and whose field it has.
    pub fn insert(&mut self, id: u64, shape: &Shape, values: &[Value]) {
        for slot in &mut self.slots {
            if !shape.labels.contains(&slot.ty) {
                continue;
            }
            if let Ok(at) = shape.keys.binary_search(&slot.field) {
                let hash = self.hasher.hash_one(&values[at]);
                slot.ids.entry(hash).or_default().insert(id);
            }
        }
    }

    /// Take node `id` (as it was stored) out of every index.
    pub fn remove(&mut self, id: u64, shape: &Shape, values: &[Value]) {
        for slot in &mut self.slots {
            if !shape.labels.contains(&slot.ty) {
                continue;
            }
            if let Ok(at) = shape.keys.binary_search(&slot.field) {
                let hash = self.hasher.hash_one(&values[at]);
                if let Some(set) = slot.ids.get_mut(&hash) {
                    set.remove(id);
                    if set.is_empty() {
                        slot.ids.remove(&hash);
                    }
                }
            }
        }
    }

    /// How many node ids the indexes hold, over all of them.
    #[cfg(test)]
    pub fn entries(&self) -> usize {
        self.slots.iter().flat_map(|slot| slot.ids.values()).map(IdSet::len).sum()
    }

    /// The ids that may hold `value` in (`ty`, `field`): None when that pair
    /// has no index, and then the caller scans.
    pub fn candidates(&self, ty: Sym, field: Sym, value: &Value) -> Option<Option<&IdSet>> {
        let slot = self.slots.iter().find(|slot| slot.ty == ty && slot.field == field)?;
        Some(slot.ids.get(&self.hasher.hash_one(value)))
    }
}
