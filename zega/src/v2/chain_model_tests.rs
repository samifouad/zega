//! Random graphs and random chain filters (zegadb/zega#86): the engine
//! with no index, the engine with every `v` indexed, and a brute-force model
//! of the grammar must return the same rows. Written by the review of #107,
//! extended with hop bands and with a schema whose fields share a kind.
use crate::graph::Graph;
use crate::value::Value;
use crate::Zega;
use serde_json::Value as Json;
use std::collections::{BTreeSet, HashMap, HashSet};

const TYPES: [&str; 6] = ["P", "T", "C", "X", "Y", "Z"];

/// Which schema a run uses: `fan` and `scout` share `knows`'s kind in the
/// second, so a step has to keep to the types its own field reaches.
static SHARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn shared() -> bool {
    SHARED.load(std::sync::atomic::Ordering::Relaxed)
}
const SCHEMA: &str = "schema {
  type P { v?: Int team: ON -> T[] knows: LINK -> P[] fan: FAN -> T[] city: IN -> C }
  type T { v?: Int players: ON <- P[] scout: SCOUT -> P[] city: IN -> C }
  type C { v?: Int people: IN <- P[] }
  type X { v?: Int nx: N1 -> X[] }
  type Y { v?: Int nx: N2 -> Z[] }
  type Z { v?: Int nx: N3 -> X[] }
}";
const SHARED_SCHEMA: &str = "schema {
  type P { v?: Int team: ON -> T[] knows: LINK -> P[] fan: LINK -> T[] city: IN -> C }
  type T { v?: Int players: ON <- P[] scout: LINK -> P[] city: IN -> C }
  type C { v?: Int people: IN <- P[] }
  type X { v?: Int nx: N1 -> Y[] }
  type Y { v?: Int nx: N2 -> Z[] }
  type Z { v?: Int nx: N3 -> X[] }
}";
fn schema() -> &'static str {
    if shared() { SHARED_SCHEMA } else { SCHEMA }
}
const INDEXES: &str = "\nindex { range P { v } range T { v } range C { v } range X { v } range Y { v } range Z { v } }";

/// (type, field) -> (kind, outgoing, target)
fn edge(ty: &str, field: &str) -> Option<(&'static str, bool, &'static str)> {
    Some(match (ty, field) {
        ("P", "team") => ("ON", true, "T"),
        ("P", "knows") => ("LINK", true, "P"),
        ("P", "fan") => (if shared() { "LINK" } else { "FAN" }, true, "T"),
        ("P", "city") => ("IN", true, "C"),
        ("T", "players") => ("ON", false, "P"),
        ("T", "scout") => (if shared() { "LINK" } else { "SCOUT" }, true, "P"),
        ("T", "city") => ("IN", true, "C"),
        ("C", "people") => ("IN", false, "P"),
        ("X", "nx") => ("N1", true, if shared() { "Y" } else { "X" }),
        ("Y", "nx") => ("N2", true, "Z"),
        ("Z", "nx") => ("N3", true, "X"),
        _ => return None,
    })
}

fn fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "P" => &["team", "knows", "fan", "city"],
        "T" => &["players", "scout", "city"],
        "C" => &["people"],
        _ => &["nx"],
    }
}

/// The repeats the checker accepts: a relationship from a type to itself.
fn repeatable(ty: &str, field: &str) -> bool {
    matches!((ty, field), ("P", "knows")) || (!shared() && (ty, field) == ("X", "nx"))
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^ (x >> 29)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }
}

struct G {
    label: HashMap<u64, &'static str>,
    v: HashMap<u64, i64>,
    rels: Vec<(&'static str, u64, u64)>,
}

fn build(rng: &mut Rng, n: u64) -> (G, Graph) {
    let mut g = G { label: HashMap::new(), v: HashMap::new(), rels: Vec::new() };
    for id in 1..=n {
        let ty = TYPES[rng.below(6) as usize];
        g.label.insert(id, ty);
        if !rng.chance(1, 6) {
            g.v.insert(id, rng.below(4) as i64);
        }
    }
    let of = |g: &G, ty: &str| -> Vec<u64> { let mut v: Vec<u64> = g.label.iter().filter(|(_, t)| **t == ty).map(|(i, _)| *i).collect(); v.sort(); v };
    let pick = |rng: &mut Rng, v: &[u64]| -> Option<u64> { if v.is_empty() { None } else { Some(v[rng.below(v.len() as u64) as usize]) } };
    let (ps, ts, cs, xs, ys, zs) = (of(&g, "P"), of(&g, "T"), of(&g, "C"), of(&g, "X"), of(&g, "Y"), of(&g, "Z"));
    let mut rels = Vec::new();
    for &p in &ps {
        for _ in 0..rng.below(3) { if let Some(t) = pick(rng, &ts) { rels.push(("ON", p, t)); } }
        for _ in 0..rng.below(4) { if let Some(q) = pick(rng, &ps) { rels.push(("LINK", p, q)); } }
        if rng.chance(1, 2) { if let Some(t) = pick(rng, &ts) { rels.push((if shared() { "LINK" } else { "FAN" }, p, t)); } }
        if rng.chance(2, 3) { if let Some(c) = pick(rng, &cs) { rels.push(("IN", p, c)); } }
        // Off-schema noise: the same kind, to a type no field reaches.
        if rng.chance(1, 8) { if let Some(x) = pick(rng, &xs) { rels.push(("LINK", p, x)); } }
    }
    for &t in &ts {
        for _ in 0..rng.below(3) { if let Some(p) = pick(rng, &ps) { rels.push((if shared() { "LINK" } else { "SCOUT" }, t, p)); } }
        if rng.chance(2, 3) { if let Some(c) = pick(rng, &cs) { rels.push(("IN", t, c)); } }
    }
    for &x in &xs { for _ in 0..1 + rng.below(2) { if let Some(y) = pick(rng, if shared() { &ys } else { &xs }) { rels.push(("N1", x, y)); } } }
    for &y in &ys { for _ in 0..1 + rng.below(2) { if let Some(z) = pick(rng, &zs) { rels.push(("N2", y, z)); } } }
    for &z in &zs { for _ in 0..rng.below(3) { if let Some(x) = pick(rng, &xs) { rels.push(("N3", z, x)); } } }
    rels.sort();
    rels.dedup();
    g.rels = rels;
    let mut graph = Graph::new();
    for id in 1..=n {
        let mut props = HashMap::new();
        if let Some(v) = g.v.get(&id) {
            props.insert("v".to_string(), Value::Int(*v));
        }
        graph.restore_node(id, vec![g.label[&id].to_string()], props);
    }
    for (i, (k, f, t)) in g.rels.iter().enumerate() {
        graph.restore_relationship(i as u64 + 1, k.to_string(), *f, *t, HashMap::new());
    }
    (g, graph)
}

#[derive(Clone, Debug)]
struct Test { ge: bool, k: i64 }

#[derive(Clone, Debug)]
struct Hop { field: &'static str, rep: Option<(usize, usize, String)>, test: Option<Test>, same: bool, word: &'static str }

#[derive(Clone, Debug)]
struct Chain { neg: bool, from: Option<(&'static str, Option<Test>)>, hops: Vec<Hop> }

#[derive(Clone, Debug)]
enum Term { Field(Test), Chain(Chain) }

fn test_text(t: &Test) -> String {
    format!("v {} {}", if t.ge { ">=" } else { "=" }, t.k)
}

fn render(q: &[Vec<Term>]) -> String {
    q.iter()
        .map(|group| {
            group
                .iter()
                .map(|term| match term {
                    Term::Field(t) => test_text(t),
                    Term::Chain(c) => {
                        let mut s = String::new();
                        if let Some((name, test)) = &c.from {
                            s.push_str("same ");
                            s.push_str(name);
                            if let Some(t) = test { s.push_str(&format!("({})", test_text(t))); }
                        } else {
                            s.push_str(if c.neg { "!have" } else { "has" });
                        }
                        for (i, h) in c.hops.iter().enumerate() {
                            if i > 0 || c.from.is_some() { s.push(' '); s.push_str(h.word); }
                            s.push(' ');
                            if h.same { s.push_str("same "); }
                            s.push_str(h.field);
                            if let Some((_, _, words)) = &h.rep {
                                s.push(' ');
                                s.push_str(words);
                            }
                            if let Some(t) = &h.test { s.push_str(&format!("({})", test_text(t))); }
                        }
                        s
                    }
                })
                .collect::<Vec<_>>()
                .join(" && ")
        })
        .collect::<Vec<_>>()
        .join(" || ")
}

fn gen_test(rng: &mut Rng) -> Test {
    Test { ge: rng.chance(1, 3), k: rng.below(4) as i64 }
}

/// A chain from `ty`; `bound` are names earlier positive chains of the
/// group reached once (name -> type).
fn gen_chain(rng: &mut Rng, ty: &'static str, bound: &HashMap<&'static str, &'static str>, allow_same: bool) -> Chain {
    let mut here = ty;
    let mut from = None;
    let mut neg = false;
    if allow_same && !bound.is_empty() && rng.chance(1, 3) {
        let mut names: Vec<_> = bound.iter().collect();
        names.sort();
        let (name, bty) = names[rng.below(names.len() as u64) as usize];
        from = Some((*name, rng.chance(1, 2).then(|| gen_test(rng))));
        here = bty;
    } else {
        neg = rng.chance(1, 4);
    }
    let len = 1 + rng.below(3) as usize;
    let mut hops = Vec::new();
    for i in 0..len {
        let fs = fields(here);
        // a join at the end
        if i + 1 == len && i > 0 && allow_same && from.is_none() && !neg && rng.chance(1, 2) {
            let options: Vec<&&'static str> = fs.iter().filter(|f| bound.get(*f).is_some_and(|bt| edge(here, f).unwrap().2 == *bt)).collect();
            if let Some(f) = options.first() {
                hops.push(Hop { field: f, rep: None, test: None, same: true, word: "in" });
                break;
            }
        }
        let field = fs[rng.below(fs.len() as u64) as usize];
        let rep = if repeatable(here, field) && rng.chance(1, 2) {
            let n = 1 + rng.below(3) as usize;
            let m = n + rng.below(2) as usize;
            Some(match rng.below(6) {
                0 => (n, n, format!("{n} hops")),
                1 => (n, n, format!("exactly {n} hops")),
                2 => (1, n, format!("within {n} hops")),
                3 => (1, n, format!("max {n} hops")),
                4 => (n, crate::lang::MAX_HOPS, format!("min {n} hops")),
                _ => (n, m, format!("min {n} max {m} hops")),
            })
        } else {
            None
        };
        let test = rng.chance(1, 2).then(|| gen_test(rng));
        hops.push(Hop { field, rep, test, same: false, word: if rng.chance(1, 2) { "in" } else { "with" } });
        here = edge(here, field).unwrap().2;
    }
    Chain { neg, from, hops }
}

fn gen_query(rng: &mut Rng, ty: &'static str) -> Vec<Vec<Term>> {
    let groups = 1 + rng.below(2) as usize;
    let mut q = Vec::new();
    for _ in 0..groups {
        let mut group = Vec::new();
        let mut bound: HashMap<&'static str, &'static str> = HashMap::new();
        let mut seen: HashMap<&'static str, usize> = HashMap::new();
        let terms = 1 + rng.below(3) as usize;
        let single = groups == 1;
        for _ in 0..terms {
            if rng.chance(1, 6) {
                group.push(Term::Field(gen_test(rng)));
                continue;
            }
            let allow_same = single && terms > 1;
            let usable: HashMap<_, _> = bound.iter().filter(|(n, _)| seen.get(*n) == Some(&1)).map(|(a, b)| (*a, *b)).collect();
            let c = gen_chain(rng, ty, &usable, allow_same);
            // names this chain reaches (for later `same`)
            let mut here = match &c.from { Some((n, _)) => usable[n], None => ty };
            for h in &c.hops {
                let next = edge(here, h.field).unwrap().2;
                if !h.same {
                    *seen.entry(h.field).or_default() += 1;
                    if !c.neg {
                        bound.insert(h.field, next);
                    }
                }
                here = next;
            }
            if c.neg {
                for h in &c.hops { seen.insert(h.field, 99); }
            }
            group.push(Term::Chain(c));
        }
        q.push(group);
    }
    q
}

// ---------------------------------------------------------------- model

fn step(g: &G, id: u64, field: &str) -> Vec<u64> {
    let ty = g.label[&id];
    let Some((kind, out, target)) = edge(ty, field) else { return Vec::new() };
    let mut v: Vec<u64> = g
        .rels
        .iter()
        .filter(|(k, f, t)| *k == kind && if out { *f == id } else { *t == id })
        .map(|(_, f, t)| if out { *t } else { *f })
        .filter(|n| g.label[n] == target)
        .collect();
    v.sort();
    v.dedup();
    v
}

fn repeat(g: &G, id: u64, field: &str, min: usize, max: usize) -> Vec<u64> {
    let mut seen = HashSet::from([id]);
    let mut layer = vec![id];
    let mut out = Vec::new();
    for depth in 1..=max {
        let mut next = Vec::new();
        for &n in &layer {
            for m in step(g, n, field) {
                if seen.insert(m) {
                    next.push(m);
                }
            }
        }
        if depth >= min {
            out.extend(next.iter().copied());
        }
        layer = next;
    }
    out
}

fn passes(g: &G, id: u64, t: &Option<Test>) -> bool {
    match t {
        None => true,
        Some(t) => g.v.get(&id).is_some_and(|v| if t.ge { *v >= t.k } else { *v == t.k }),
    }
}

/// Every binding (hop name -> node) of a walk of `c` from `root` that passes.
fn paths(g: &G, root: u64, c: &Chain, bound: &HashMap<&'static str, u64>) -> Vec<HashMap<&'static str, u64>> {
    let start = match &c.from {
        None => root,
        Some((name, test)) => {
            let Some(&n) = bound.get(name) else { return Vec::new() };
            if !passes(g, n, test) {
                return Vec::new();
            }
            n
        }
    };
    let mut out = Vec::new();
    fn rec(g: &G, c: &Chain, k: usize, at: u64, b: &mut HashMap<&'static str, u64>, bound: &HashMap<&'static str, u64>, out: &mut Vec<HashMap<&'static str, u64>>) {
        let Some(h) = c.hops.get(k) else {
            out.push(b.clone());
            return;
        };
        let next = match h.rep {
            None => step(g, at, h.field),
            Some((min, max, _)) => repeat(g, at, h.field, min, max),
        };
        for m in next {
            if !passes(g, m, &h.test) {
                continue;
            }
            if h.same && bound.get(h.field) != Some(&m) {
                continue;
            }
            let prev = if h.same { None } else { b.insert(h.field, m) };
            rec(g, c, k + 1, m, b, bound, out);
            if !h.same {
                match prev { Some(p) => { b.insert(h.field, p); } None => { b.remove(h.field); } }
            }
        }
    }
    rec(g, c, 0, start, &mut HashMap::new(), bound, &mut out);
    out
}

fn group_holds(g: &G, id: u64, terms: &[Term]) -> bool {
    let mut positive = Vec::new();
    for t in terms {
        match t {
            Term::Field(t) => if !passes(g, id, &Some(t.clone())) { return false },
            Term::Chain(c) if c.neg => if !paths(g, id, c, &HashMap::new()).is_empty() { return false },
            Term::Chain(c) => positive.push(c),
        }
    }
    fn solve(g: &G, id: u64, cs: &[&Chain], bound: &mut HashMap<&'static str, u64>) -> bool {
        let Some((c, rest)) = cs.split_first() else { return true };
        for b in paths(g, id, c, bound) {
            let saved = bound.clone();
            bound.extend(b);
            if solve(g, id, rest, bound) {
                return true;
            }
            *bound = saved;
        }
        false
    }
    solve(g, id, &positive, &mut HashMap::new())
}

fn model(g: &G, ty: &str, q: &[Vec<Term>]) -> Vec<u64> {
    let mut ids: Vec<u64> = g.label.iter().filter(|(_, t)| **t == ty).map(|(i, _)| *i).collect();
    ids.sort();
    ids.retain(|id| q.iter().any(|group| group_holds(g, *id, group)));
    ids
}

fn ids_of(result: &Json) -> Vec<u64> {
    let rows = match result {
        Json::Array(rows) => rows.clone(),
        other => vec![other.clone()],
    };
    rows.iter().map(|r| r["@id"].as_u64().or_else(|| r["id"].as_u64()).unwrap_or_else(|| panic!("row {r}"))).collect()
}

fn run(seed: u64, nodes: u64, queries: usize, stats: &mut [usize; 4]) {
    let mut rng = Rng(seed);
    let (g, graph) = build(&mut rng, nodes);
    let bytes = crate::wal::encode_snapshot(&graph).unwrap();
    let with = Zega::in_memory().build().unwrap();
    with.restore_bytes(&bytes).unwrap();
    let without = Zega::in_memory().build().unwrap();
    without.restore_bytes(&bytes).unwrap();
    let schema_idx = format!("{}{INDEXES}", schema());
    for _ in 0..queries {
        let ty = ["P", "T", "C", "X"][rng.below(4) as usize];
        let q = gen_query(&mut rng, ty);
        if !q.iter().flatten().any(|t| matches!(t, Term::Chain(_))) {
            continue;
        }
        let text = format!("{{ {ty}({}) {{ @id }} }}", render(&q));
        let plain = without.run_lang(schema(), &text);
        let indexed = with.run_lang(&schema_idx, &text);
        let want = model(&g, ty, &q);
        match (&plain, &indexed) {
            (Ok(a), Ok(b)) => {
                stats[0] += 1;
                let (a, b) = (ids_of(a), ids_of(b));
                let plain_ok = a == want;
                let index_ok = b == want;
                if !plain_ok { stats[2] += 1; }
                if !index_ok { stats[3] += 1; }
                if !plain_ok || !index_ok {
                    let only = |x: &[u64], y: &[u64]| -> Vec<u64> { let y: BTreeSet<_> = y.iter().collect(); x.iter().filter(|i| !y.contains(i)).copied().collect() };
                    eprintln!("MISMATCH seed {seed}: {text}\n  model {want:?}\n  plain {a:?} (extra {:?} missing {:?})\n  index {b:?} (extra {:?} missing {:?})", only(&a, &want), only(&want, &a), only(&b, &want), only(&want, &b));
                }
            }
            (Err(a), Err(b)) => {
                stats[1] += 1;
                assert_eq!(a.to_string(), b.to_string(), "{text}");
            }
            _ => panic!("one side failed: {text}\n{plain:?}\n{indexed:?}"),
        }
    }
}

fn check(shared_kind: bool) {
    SHARED.store(shared_kind, std::sync::atomic::Ordering::Relaxed);
    let mut stats = [0usize; 4];
    for seed in 1..=40u64 {
        run(seed * 0x9e37_79b9, 20 + (seed % 5) * 10, 150, &mut stats);
    }
    eprintln!("ran {} queries, {} refused by the checker; mismatches plain {} indexed {}", stats[0], stats[1], stats[2], stats[3]);
    assert!(stats[0] > 1000);
    assert_eq!((stats[2], stats[3]), (0, 0));
}

/// One test, both schemas in turn: the schema is a global the model reads.
#[test]
fn chains_match_a_brute_force_model_with_and_without_indexes() {
    check(false);
    check(true);
}
