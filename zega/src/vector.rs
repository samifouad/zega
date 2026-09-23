//! Float32 vectors and deterministic, in-tree HNSW (Malkov & Yashunin, 2018).
//! Scores are always larger-is-better: cosine, dot product, or negative L2.
use crate::graph::NodeId;
use serde::{Deserialize, Serialize};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    #[default]
    Cosine,
    Dot,
    L2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VectorSpec {
    pub dimensions: usize,
    pub metric: Metric,
}
impl VectorSpec {
    pub fn parse(ty: &str) -> Option<Self> {
        let body = ty.strip_prefix("Vector<")?.strip_suffix('>')?;
        let mut parts = body.split(',');
        let dimensions = parts.next()?.trim().parse().ok()?;
        let metric = match parts.next().unwrap_or("cosine").trim() {
            "cosine" => Metric::Cosine,
            "dot" => Metric::Dot,
            "l2" => Metric::L2,
            _ => return None,
        };
        ((1..=4096).contains(&dimensions) && parts.next().is_none())
            .then_some(Self { dimensions, metric })
    }
    pub fn value(self, json: &serde_json::Value) -> Result<Vector, String> {
        let v = Vector::from_json(json, self.metric)?;
        if v.bits.len() != self.dimensions {
            return Err(format!(
                "Vector<{}> needs exactly {} numbers, got {}",
                self.dimensions,
                self.dimensions,
                v.bits.len()
            ));
        }
        Ok(v)
    }
}

/// Bit storage preserves float32 values in equality, WAL and snapshots.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Vector {
    bits: Vec<u32>,
    pub metric: Metric,
}
impl Vector {
    pub fn new(values: &[f32], metric: Metric) -> Result<Self, String> {
        if !(1..=4096).contains(&values.len()) {
            return Err("Vector dimension must be in 1..=4096".into());
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err("Vector components must be finite float32 numbers".into());
        }
        Ok(Self {
            bits: values
                .iter()
                .map(|&v| (if v == 0.0 { 0.0 } else { v }).to_bits())
                .collect(),
            metric,
        })
    }
    pub fn from_json(json: &serde_json::Value, metric: Metric) -> Result<Self, String> {
        let values = json
            .as_array()
            .ok_or("Vector needs an array of numbers")?
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|v| v.is_finite() && (*v as f32).is_finite())
                    .map(|v| v as f32)
                    .ok_or_else(|| "Vector components must be finite float32 numbers".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(&values, metric)
    }
    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.bits.iter().map(|&v| f64::from(f32::from_bits(v)))
    }
    pub fn dimensions(&self) -> usize {
        self.bits.len()
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!(self.values().collect::<Vec<_>>())
    }
    pub fn score(&self, other: &Self) -> Option<f64> {
        if self.dimensions() != other.dimensions() {
            return None;
        }
        let (mut dot, mut a, mut b, mut l2) = (0.0, 0.0, 0.0, 0.0);
        for (x, y) in self.values().zip(other.values()) {
            dot += x * y;
            a += x * x;
            b += y * y;
            l2 += (x - y) * (x - y);
        }
        Some(match self.metric {
            Metric::Cosine => {
                if a == 0.0 || b == 0.0 {
                    0.0
                } else {
                    (dot / (a * b).sqrt()).clamp(-1.0, 1.0)
                }
            }
            Metric::Dot => dot,
            Metric::L2 => -l2.sqrt(),
        })
    }
}

#[derive(Clone)]
struct Entry {
    id: NodeId,
    vector: Vector,
    links: Vec<Vec<usize>>,
    live: bool,
}
#[derive(Clone, Copy, Debug)]
struct Candidate {
    distance: f64,
    slot: usize,
}
impl PartialEq for Candidate {
    fn eq(&self, b: &Self) -> bool {
        self.cmp(b).is_eq()
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}
impl Ord for Candidate {
    fn cmp(&self, b: &Self) -> Ordering {
        self.distance
            .total_cmp(&b.distance)
            .then(self.slot.cmp(&b.slot))
    }
}

/// Incremental multi-layer graph. Deleted slots remain routing nodes until
/// compaction; they are never returned. Updates insert a new slot.
#[derive(Clone)]
pub struct Hnsw {
    entries: Vec<Entry>,
    live: HashMap<NodeId, usize>,
    entry: Option<usize>,
    seed: u64,
}
const M: usize = 24;
const EF_CONSTRUCTION: usize = 160;
const EF_SEARCH: usize = 768;
impl Hnsw {
    pub fn new(seed: u64) -> Self {
        Self {
            entries: Vec::new(),
            live: HashMap::new(),
            entry: None,
            seed,
        }
    }
    fn candidate(&self, q: &Vector, slot: usize) -> Candidate {
        Candidate {
            distance: -self.entries[slot]
                .vector
                .score(q)
                .expect("index dimensions"),
            slot,
        }
    }
    fn layer(&self, q: &Vector, entry: usize, level: usize, ef: usize) -> Vec<Candidate> {
        let first = self.candidate(q, entry);
        let mut todo = BinaryHeap::from([Reverse(first)]);
        let mut best = BinaryHeap::from([first]);
        let mut seen = HashSet::from([entry]);
        while let Some(Reverse(next)) = todo.pop() {
            if best.len() >= ef && next > *best.peek().unwrap() {
                break;
            }
            for &slot in &self.entries[next.slot].links[level] {
                if !seen.insert(slot) {
                    continue;
                }
                let c = self.candidate(q, slot);
                if best.len() < ef || c < *best.peek().unwrap() {
                    todo.push(Reverse(c));
                    best.push(c);
                    if best.len() > ef {
                        best.pop();
                    }
                }
            }
        }
        best.into_sorted_vec()
    }
    fn select(&self, candidates: &[Candidate], count: usize) -> Vec<usize> {
        let mut selected: Vec<usize> = Vec::new();
        for c in candidates {
            if selected.iter().all(|&s| {
                -self.entries[c.slot]
                    .vector
                    .score(&self.entries[s].vector)
                    .unwrap()
                    >= c.distance
            }) {
                selected.push(c.slot);
                if selected.len() == count {
                    return selected;
                }
            }
        }
        // Retain pruned candidates when the diversity heuristic leaves spare links.
        for c in candidates {
            if !selected.contains(&c.slot) {
                selected.push(c.slot);
                if selected.len() == count {
                    break;
                }
            }
        }
        selected
    }
    pub fn insert(&mut self, id: NodeId, vector: Vector) {
        self.remove(id);
        let mut random = id ^ self.seed;
        random = (random ^ (random >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        random = (random ^ (random >> 27)).wrapping_mul(0x94d049bb133111eb);
        random ^= random >> 31;
        let mut level = 0;
        while random.is_multiple_of(M as u64) && level < 12 {
            level += 1;
            random /= M as u64;
        }
        let slot = self.entries.len();
        let Some(mut ep) = self.entry else {
            self.entries.push(Entry {
                id,
                vector,
                links: vec![vec![]; level + 1],
                live: true,
            });
            self.live.insert(id, slot);
            self.entry = Some(slot);
            return;
        };
        let top = self.entries[ep].links.len() - 1;
        for l in ((level + 1)..=top).rev() {
            ep = self.layer(&vector, ep, l, 1)[0].slot;
        }
        let mut links = vec![vec![]; level + 1];
        for l in (0..=level.min(top)).rev() {
            let candidates = self.layer(&vector, ep, l, EF_CONSTRUCTION);
            links[l] = self.select(&candidates, M);
            ep = candidates[0].slot;
        }
        self.entries.push(Entry {
            id,
            vector,
            links: links.clone(),
            live: true,
        });
        self.live.insert(id, slot);
        for (l, neighbors) in links.iter().enumerate() {
            let max = if l == 0 { M * 2 } else { M };
            for &n in neighbors {
                self.entries[n].links[l].push(slot);
                if self.entries[n].links[l].len() > max {
                    let q = &self.entries[n].vector;
                    let mut candidates: Vec<_> = self.entries[n].links[l]
                        .iter()
                        .map(|&s| self.candidate(q, s))
                        .collect();
                    candidates.sort_unstable();
                    let selected = self.select(&candidates, max);
                    self.entries[n].links[l] = selected;
                }
            }
        }
        if level > top {
            self.entry = Some(slot);
        }
    }
    pub fn remove(&mut self, id: NodeId) {
        if let Some(slot) = self.live.remove(&id) {
            self.entries[slot].live = false;
        }
        if self.entries.len() > 64 && self.entries.len() > self.live.len() * 2 {
            let mut live: Vec<_> = self
                .live
                .values()
                .map(|&s| (self.entries[s].id, self.entries[s].vector.clone()))
                .collect();
            live.sort_by_key(|(id, _)| *id);
            let mut rebuilt = Self::new(self.seed);
            for (id, v) in live {
                rebuilt.insert(id, v);
            }
            *self = rebuilt;
        }
    }
    pub fn nearest(
        &self,
        q: &Vector,
        k: usize,
        exact: bool,
        allowed: &impl Fn(NodeId) -> bool,
    ) -> Vec<(NodeId, f64)> {
        if k == 0 || self.live.is_empty() {
            return Vec::new();
        }
        let mut ep = self.entry.unwrap();
        if self.entries[ep].vector.dimensions() != q.dimensions() {
            return Vec::new();
        }
        let rank = |slots: Vec<usize>| {
            let mut scores: Vec<_> = slots
                .into_iter()
                .filter(|&s| self.entries[s].live && allowed(self.entries[s].id))
                .map(|s| (self.entries[s].id, self.entries[s].vector.score(q).unwrap()))
                .collect();
            scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            scores.truncate(k);
            scores
        };
        if exact {
            return rank(self.live.values().copied().collect());
        }
        for l in (1..self.entries[ep].links.len()).rev() {
            ep = self.layer(q, ep, l, 1)[0].slot;
        }
        let mut ef = EF_SEARCH.max(k).min(self.entries.len());
        loop {
            let result = rank(
                self.layer(q, ep, 0, ef)
                    .into_iter()
                    .map(|c| c.slot)
                    .collect(),
            );
            if result.len() >= k || ef == self.entries.len() {
                return result;
            }
            ef = (ef * 2).min(self.entries.len());
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct VectorIndex {
    fields: HashMap<(String, usize, Metric), Hnsw>,
}
impl VectorIndex {
    pub fn insert(&mut self, field: &str, v: &Vector, id: NodeId) {
        self.fields
            .entry((field.into(), v.dimensions(), v.metric))
            .or_insert_with(|| Hnsw::new(0x5e6a))
            .insert(id, v.clone());
    }
    pub fn remove(&mut self, field: &str, v: &Vector, id: NodeId) {
        if let Some(index) = self
            .fields
            .get_mut(&(field.into(), v.dimensions(), v.metric))
        {
            index.remove(id);
        }
    }
    pub fn nearest(
        &self,
        field: &str,
        q: &Vector,
        k: usize,
        exact: bool,
        allowed: impl Fn(NodeId) -> bool,
    ) -> Vec<(NodeId, f64)> {
        self.fields
            .get(&(field.into(), q.dimensions(), q.metric))
            .map(|i| i.nearest(q, k, exact, &allowed))
            .unwrap_or_default()
    }
}

/// Deterministic centered PCA, using covariance multiplication without a D²
/// allocation. Fixed starts, Gram-Schmidt deflation and sign convention make
/// the three axes reproducible. Degenerate components remain zero.
pub fn pca(vectors: &[Vector]) -> Vec<[f64; 3]> {
    if vectors.is_empty() {
        return Vec::new();
    }
    let d = vectors[0].dimensions();
    assert!(vectors.iter().all(|v| v.dimensions() == d));
    let mut mean = vec![0.0; d];
    for v in vectors {
        for (m, x) in mean.iter_mut().zip(v.values()) {
            *m += x / vectors.len() as f64;
        }
    }
    let rows: Vec<Vec<_>> = vectors
        .iter()
        .map(|v| v.values().zip(&mean).map(|(x, m)| x - m).collect())
        .collect();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
    let mut axes: Vec<Vec<f64>> = Vec::new();
    for axis in 0..3.min(d) {
        let mut v: Vec<_> = (0..d)
            .map(|j| (((j + 1) * (axis + 3)) as f64).sin())
            .collect();
        for _ in 0..128 {
            let mut next = vec![0.0; d];
            for row in &rows {
                let weight = dot(row, &v);
                for (x, r) in next.iter_mut().zip(row) {
                    *x += weight * r;
                }
            }
            for prev in &axes {
                let weight = dot(&next, prev);
                for (x, p) in next.iter_mut().zip(prev) {
                    *x -= weight * p;
                }
            }
            let norm = dot(&next, &next).sqrt();
            if norm <= 1e-20 {
                v.fill(0.0);
                break;
            }
            for x in &mut next {
                *x /= norm;
            }
            let change: f64 = v.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
            v = next;
            if change < 1e-10 {
                break;
            }
        }
        if let Some((i, _)) = v
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()).then(b.0.cmp(&a.0)))
        {
            if v[i] < 0.0 {
                for x in &mut v {
                    *x = -*x;
                }
            }
        }
        axes.push(v);
    }
    rows.iter()
        .map(|row| {
            let mut p = [0.0; 3];
            for (i, a) in axes.iter().enumerate() {
                p[i] = dot(row, a);
            }
            p
        })
        .collect()
}
