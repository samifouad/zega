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
    /// The float32 components as stored: canonical bits (no `-0.0`, all
    /// finite). The `.graph` writer encodes exactly these.
    pub(crate) fn bits(&self) -> &[u32] {
        &self.bits
    }

    /// Rebuild a vector from stored bits, accepting only what [`Vector::new`]
    /// itself produces, so every stored vector has one encoding.
    pub(crate) fn from_bits(bits: Vec<u32>, metric: Metric) -> Result<Self, String> {
        let values: Vec<f32> = bits.iter().map(|&bits| f32::from_bits(bits)).collect();
        let vector = Self::new(&values, metric)?;
        if vector.bits != bits {
            return Err("Vector component -0.0 must be stored as 0.0".into());
        }
        Ok(vector)
    }

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

#[cfg(test)]
mod vector_search_width {
    use super::{Hnsw, Metric, Vector};
    use std::time::Instant;

    fn cosine(a: &[f32], b: &[f32]) -> f64 {
        let (mut dot, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for (&x, &y) in a.iter().zip(b) {
            dot += f64::from(x) * f64::from(y);
            aa += f64::from(x).powi(2);
            bb += f64::from(y).powi(2);
        }
        (dot / (aa * bb).sqrt()).clamp(-1.0, 1.0)
    }
    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed >> 40) as f32 / 16777216.0) * 2.0 - 1.0
    }

    /// Seeded rows and held-out queries with brute-force top-10 IDs.
    struct Corpus {
        name: &'static str,
        rows: Vec<Vec<f32>>,
        queries: Vec<Vec<f32>>,
        truth: Vec<Vec<u64>>,
        /// Mean cosine distance over the 10th-nearest distance, averaged over
        /// queries (He, Kumar and Chang, 2012). Near 1, neighbours are barely
        /// nearer than anything else and no index can skip much of the data.
        contrast: f64,
    }
    /// Uniform in [-1, 1]^d, or 40 centres with ±0.08 noise per coordinate.
    fn corpus(clustered: bool, n: usize, d: usize, q: usize) -> Corpus {
        let mut seed = 0x123456789abcdefu64 ^ ((n as u64) << 20) ^ d as u64;
        let centers: Vec<Vec<f32>> = (0..40)
            .map(|_| (0..d).map(|_| rng(&mut seed)).collect())
            .collect();
        let mut row = |i: usize| -> Vec<f32> {
            if clustered {
                let c = &centers[i % centers.len()];
                c.iter().map(|&x| x + rng(&mut seed) * 0.08).collect()
            } else {
                (0..d).map(|_| rng(&mut seed)).collect()
            }
        };
        let rows: Vec<_> = (0..n).map(&mut row).collect();
        let queries: Vec<_> = (0..q).map(&mut row).collect();
        let exact: Vec<(Vec<u64>, f64)> = std::thread::scope(|s| {
            let rows = &rows;
            let handles: Vec<_> = queries
                .chunks(q.div_ceil(8))
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .map(|query| {
                                let mut ranked: Vec<_> = rows
                                    .iter()
                                    .enumerate()
                                    .map(|(id, r)| (id as u64, cosine(r, query)))
                                    .collect();
                                let mean =
                                    ranked.iter().map(|(_, c)| 1.0 - c).sum::<f64>() / n as f64;
                                ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                                let contrast = mean / (1.0 - ranked[9].1);
                                (
                                    ranked.iter().take(10).map(|(id, _)| *id).collect(),
                                    contrast,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        Corpus {
            name: if clustered { "40-cluster" } else { "uniform" },
            rows,
            queries,
            contrast: exact.iter().map(|(_, c)| c).sum::<f64>() / q as f64,
            truth: exact.into_iter().map(|(ids, _)| ids).collect(),
        }
    }
    struct Measured {
        recall: f64,
        /// Distance computations per query as a fraction of N.
        visited: f64,
        worst_visited: f64,
        ms_per_query: f64,
        exact_ms_per_query: f64,
        build_s: f64,
    }
    fn measure(c: &Corpus) -> Measured {
        let start = Instant::now();
        let mut index = Hnsw::new(0x5e6a);
        for (id, row) in c.rows.iter().enumerate() {
            index.insert(id as u64, Vector::new(row, Metric::Cosine).unwrap());
        }
        let build_s = start.elapsed().as_secs_f64();
        let queries: Vec<_> = c
            .queries
            .iter()
            .map(|q| Vector::new(q, Metric::Cosine).unwrap())
            .collect();
        let start = Instant::now();
        let found: Vec<_> = queries
            .iter()
            .map(|q| index.search(q, 10, &|_| true))
            .collect();
        let hnsw_elapsed = start.elapsed();
        let (mut hits, mut visited, mut worst) = (0, 0, 0);
        for ((q, (got, v)), want) in queries.iter().zip(&found).zip(&c.truth) {
            assert_eq!(got, &index.nearest(q, 10, false, &|_| true));
            visited += v;
            worst = worst.max(*v);
            hits += got.iter().filter(|(id, _)| want.contains(id)).count();
        }
        let start = Instant::now();
        for q in &queries {
            std::hint::black_box(index.nearest(q, 10, true, &|_| true));
        }
        let exact_elapsed = start.elapsed();
        let (q, n) = (c.queries.len() as f64, c.rows.len() as f64);
        let m = Measured {
            recall: hits as f64 / (q * 10.0),
            visited: visited as f64 / (q * n),
            worst_visited: worst as f64 / n,
            ms_per_query: hnsw_elapsed.as_secs_f64() * 1000.0 / q,
            exact_ms_per_query: exact_elapsed.as_secs_f64() * 1000.0 / q,
            build_s,
        };
        eprintln!(
            "| {} | {} | {} | {:.2} | {:.4} | {:.2}% | {:.1}% | {:.3} | {:.3} | {:.1} |",
            c.rows.len(),
            c.rows[0].len(),
            c.name,
            c.contrast,
            m.recall,
            m.visited * 100.0,
            m.worst_visited * 100.0,
            m.ms_per_query,
            m.exact_ms_per_query,
            m.build_s,
        );
        m
    }
    fn table(sizes: &[usize], dims: &[usize]) {
        eprintln!("| N | dims | data | contrast | recall@10 | scored/N (mean) | worst query | ms/query | exact scan ms/query | build s |");
        eprintln!("|---|---|---|---|---|---|---|---|---|---|");
        // One index at a time, so latency is not shared with other builds.
        for &n in sizes {
            for &d in dims {
                for clustered in [false, true] {
                    measure(&corpus(clustered, n, d, 100));
                }
            }
        }
    }

    /// Fails with main's fixed search width of 768 (zega#26), which scored
    /// ~87% of N per query on the uniform corpus and ~32% on the clustered one.
    /// Sized to run in well under a minute in a debug build.
    #[test]
    fn vector_hnsw_search_width_6000_by_16() {
        std::thread::scope(|s| {
            for (clustered, max_visited) in [(false, 0.45), (true, 0.08)] {
                s.spawn(move || {
                    let c = corpus(clustered, 6_000, 16, 30);
                    let m = measure(&c);
                    assert!(m.recall >= 0.95, "{} recall@10={}", c.name, m.recall);
                    assert!(
                        m.visited <= max_visited,
                        "{} scored {:.1}% of N per query",
                        c.name,
                        m.visited * 100.0
                    );
                });
            }
        });
    }

    #[test]
    #[ignore = "local benchmark: builds 12 indexes of up to 100,000 x 384"]
    fn vector_hnsw_benchmark_10k_100k() {
        table(&[10_000, 100_000], &[32, 128, 384]);
    }

    #[test]
    #[ignore = "local benchmark: builds 1,000,000-vector indexes (hours)"]
    fn vector_hnsw_benchmark_1m() {
        table(&[1_000_000], &[32, 128]);
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
/// First search width; it grows with k and doubles until the top k settle.
const EF_START: usize = 64;

/// Resumable bounded best-first search of one layer (the paper's SEARCH-LAYER).
/// Every score is computed at most once per query; widening keeps the work
/// already done, so a wider pass only explores what the narrower one skipped.
struct Beam<'a> {
    index: &'a Hnsw,
    q: &'a Vector,
    level: usize,
    /// Negated scores by slot: every slot reached, and the distance
    /// computations made for this query.
    scores: HashMap<usize, f64>,
    expanded: HashSet<usize>,
    todo: BinaryHeap<Reverse<Candidate>>,
    best: BinaryHeap<Candidate>,
    /// Scored candidates outside `best`, readmitted when the beam widens.
    spill: Vec<Candidate>,
}
impl<'a> Beam<'a> {
    fn new(index: &'a Hnsw, q: &'a Vector, level: usize, entry: usize) -> Self {
        let mut beam = Self {
            index,
            q,
            level,
            scores: HashMap::new(),
            expanded: HashSet::new(),
            todo: BinaryHeap::new(),
            best: BinaryHeap::new(),
            spill: Vec::new(),
        };
        let first = beam.candidate(entry);
        beam.todo.push(Reverse(first));
        beam.best.push(first);
        beam
    }
    fn candidate(&mut self, slot: usize) -> Candidate {
        let (entries, q) = (&self.index.entries, self.q);
        let distance = *self
            .scores
            .entry(slot)
            .or_insert_with(|| -entries[slot].vector.score(q).expect("index dimensions"));
        Candidate { distance, slot }
    }
    fn admit(&mut self, c: Candidate, ef: usize) {
        if self.best.len() < ef || c < *self.best.peek().unwrap() {
            self.todo.push(Reverse(c));
            self.best.push(c);
            if self.best.len() > ef {
                self.spill.push(self.best.pop().unwrap());
            }
        } else {
            self.spill.push(c);
        }
    }
    /// Searches until the `ef` nearest found so far cannot improve.
    fn run(&mut self, ef: usize) -> Vec<Candidate> {
        if self.best.len() < ef && !self.spill.is_empty() {
            let room = (ef - self.best.len()).min(self.spill.len());
            if room < self.spill.len() {
                // Candidates are totally ordered, so the admitted set is unique.
                self.spill.select_nth_unstable(room);
            }
            for c in self.spill.drain(..room) {
                self.todo.push(Reverse(c));
                self.best.push(c);
            }
        }
        while let Some(Reverse(next)) = self.todo.pop() {
            if self.best.len() >= ef && next > *self.best.peek().unwrap() {
                self.todo.push(Reverse(next));
                break;
            }
            if !self.expanded.insert(next.slot) {
                continue;
            }
            let index = self.index;
            for &slot in &index.entries[next.slot].links[self.level] {
                if !self.scores.contains_key(&slot) {
                    let c = self.candidate(slot);
                    self.admit(c, ef);
                }
            }
        }
        let mut found: Vec<_> = self.best.iter().copied().collect();
        found.sort_unstable();
        found
    }
}

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
        Beam::new(self, q, level, entry).run(ef)
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
        if exact {
            self.scan(q, k, allowed, &HashMap::new()).0
        } else {
            self.search(q, k, allowed).0
        }
    }
    /// Exact top k over the eligible live entries, reusing scores already
    /// computed; also returns how many scores it had to compute.
    fn scan(
        &self,
        q: &Vector,
        k: usize,
        allowed: &impl Fn(NodeId) -> bool,
        scored: &HashMap<usize, f64>,
    ) -> (Vec<(NodeId, f64)>, usize) {
        if k == 0 || !self.accepts(q) {
            return (Vec::new(), 0);
        }
        let mut computed = 0;
        let mut scores: Vec<_> = self
            .live
            .values()
            .filter(|&&s| allowed(self.entries[s].id))
            .map(|&s| {
                let score = scored.get(&s).map(|d| -d).unwrap_or_else(|| {
                    computed += 1;
                    self.entries[s].vector.score(q).unwrap()
                });
                (self.entries[s].id, score)
            })
            .collect();
        rank(&mut scores, k);
        (scores, computed)
    }
    fn accepts(&self, q: &Vector) -> bool {
        self.entry.is_some_and(|ep| {
            !self.live.is_empty() && self.entries[ep].vector.dimensions() == q.dimensions()
        })
    }
    /// Approximate nearest k, and how many distance computations it made.
    ///
    /// The search width starts at max(k, EF_START) and doubles until two
    /// successive widths return the same top k, so the width tracks how hard
    /// this query is on this data rather than a constant. Once a search has
    /// scored half of the live entries, a wider one would cost as much as a
    /// scan, so the scan finishes it exactly with the scores already computed.
    pub(crate) fn search(
        &self,
        q: &Vector,
        k: usize,
        allowed: &impl Fn(NodeId) -> bool,
    ) -> (Vec<(NodeId, f64)>, usize) {
        let Some(mut ep) = self.entry.filter(|_| k > 0 && self.accepts(q)) else {
            return (Vec::new(), 0);
        };
        let mut upper = 0;
        for l in (1..self.entries[ep].links.len()).rev() {
            let mut beam = Beam::new(self, q, l, ep);
            ep = beam.run(1)[0].slot;
            upper += beam.scores.len();
        }
        let n = self.entries.len();
        let mut ef = EF_START.max(k).min(n);
        let mut beam = Beam::new(self, q, 0, ep);
        let mut previous = None;
        loop {
            let mut result: Vec<_> = beam
                .run(ef)
                .into_iter()
                .filter(|c| self.entries[c.slot].live && allowed(self.entries[c.slot].id))
                .map(|c| (self.entries[c.slot].id, -c.distance))
                .collect();
            rank(&mut result, k);
            let ids: Vec<NodeId> = result.iter().map(|(id, _)| *id).collect();
            let settled = result.len() == k && previous.as_ref() == Some(&ids);
            if settled || ef == n {
                return (result, upper + beam.scores.len());
            }
            if beam.scores.len() * 2 >= self.live.len() {
                let (exact, computed) = self.scan(q, k, allowed, &beam.scores);
                return (exact, upper + beam.scores.len() + computed);
            }
            previous = (result.len() == k).then_some(ids);
            ef = (ef * 2).min(n);
        }
    }
}

/// Larger score first, ties by ascending node ID; keeps the first k.
fn rank(scores: &mut Vec<(NodeId, f64)>, k: usize) {
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scores.truncate(k);
}

#[derive(Clone, Default)]
pub(crate) struct VectorIndex {
    fields: HashMap<(String, usize, Metric), Hnsw>,
}
impl VectorIndex {
    /// Each index's node ids in the order they were inserted, which with
    /// the ids fixes the whole HNSW graph.
    #[cfg(test)]
    pub fn insertion_order(&self) -> Vec<(String, Vec<NodeId>)> {
        let mut out: Vec<_> = self
            .fields
            .iter()
            .map(|((field, dims, metric), index)| {
                (format!("{field}/{dims}/{metric:?}"), index.entries.iter().map(|e| e.id).collect())
            })
            .collect();
        out.sort();
        out
    }

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
