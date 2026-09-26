//! Tests from the review of zegadb/zega#114: ids in the last chunk (the
//! one that ends at `u64::MAX`, review H1) through every way a graph gets
//! them, `IdMap` against a model with checks after every step, and unique
//! indexes that hold exactly one entry per indexed node after any writes.
use super::*;
use super::idmap::IdMap;

#[test]
fn idmap_first_id_in_last_chunk() {
    let mut map = IdMap::default();
    map.insert(u64::MAX - 1, 1u8);
    assert_eq!(map.get(u64::MAX - 1), Some(&1));
}

#[test]
fn graph_node_in_last_chunk() {
    let mut g = Graph::new();
    g.restore_node(u64::MAX - 1, vec!["T".into()], HashMap::new());
    assert!(g.get_node(u64::MAX - 1).is_some());
}

/// A crafted legacy snapshot (the wasm explorer's import path).
#[test]
fn snapshot_with_node_in_last_chunk() {
    let id = u64::MAX - 1;
    let node = Node { id, labels: vec!["T".into()], props: HashMap::new() };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&bincode::serialize(&id).unwrap());
    bytes.extend_from_slice(&bincode::serialize(&node).unwrap());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    let mut g = Graph::new();
    crate::wal::restore_bytes(&mut g, &bytes).unwrap();
    assert!(g.get_node(id).is_some());
}

/// IdMap against a BTreeMap with ids in every region, heavy deletes, and
/// checks after every step.
#[test]
fn idmap_model_dense_check() {
    use std::collections::BTreeMap;
    for seed in 1..=30u64 {
        let mut map = IdMap::default();
        let mut model = BTreeMap::new();
        let mut s = seed.wrapping_mul(0x9e3779b97f4a7c15);
        let mut next = 0u64;
        for step in 0..6_000u64 {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = s >> 11;
            let roll = x % 100;
            let id = match roll % 10 {
                0..=4 => { next += 1; next }
                5 => next.saturating_sub(x % 3000),
                6 => (1u64 << 40) + (x % 5000),
                7 => next + 1024 * (x % 200),
                8 => x % (next + 1),
                _ => (1u64 << 20) + (x % 3000),
            };
            if roll < 55 {
                assert_eq!(map.insert(id, step), model.insert(id, step));
            } else {
                // delete a random live id or the id
                let target = if roll < 90 { model.keys().nth((x as usize) % model.len().max(1)).copied().unwrap_or(id) } else { id };
                assert_eq!(map.remove(target), model.remove(&target));
            }
            assert_eq!(map.len(), model.len());
            if step % 7 == 0 {
                assert!(map.iter().map(|(i, v)| (i, *v)).eq(model.iter().map(|(i, v)| (*i, *v))), "seed {seed} step {step}");
            }
            for probe in [id, id.wrapping_add(1), next] {
                assert_eq!(map.get(probe), model.get(&probe));
            }
        }
    }
}

/// The unique indexes hold exactly one entry per (declared pair, node with
/// that label and field): nothing stale after any sequence of writes.
#[test]
fn unique_entries_are_exact() {
    let pairs: Vec<(String, String)> = vec![("A".into(), "k".into()), ("B".into(), "k".into()), ("A".into(), "j".into())];
    let mut s = 7u64;
    let mut g = Graph::new();
    g.sync_uniques(&pairs);
    let mut live: Vec<NodeId> = Vec::new();
    for step in 0..20_000u64 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let x = s >> 13;
        let labels: Vec<String> = match x % 5 { 0 => vec![], 1 => vec!["A".into()], 2 => vec!["B".into()], 3 => vec!["A".into(), "B".into()], _ => vec!["A".into(), "A".into()] };
        let v = match (x >> 4) % 6 { 0 => Value::from_f64(f64::NAN), 1 => Value::from_f64(-0.0), 2 => Value::from_f64(0.0), 3 => Value::Int(0), 4 => Value::List(vec![Value::Int((x % 3) as i64)].into()), _ => Value::Int((x % 7) as i64) };
        let mut props = HashMap::new();
        if !(x >> 8).is_multiple_of(3) { props.insert("k".to_string(), v.clone()); }
        if (x >> 9).is_multiple_of(2) { props.insert("j".to_string(), v.clone()); }
        match (x >> 12) % 6 {
            0 | 1 => live.push(g.create_node(labels, props)),
            2 if !live.is_empty() => { let id = live[(x as usize >> 3) % live.len()]; g.restore_node(id, labels, props); }
            3 if !live.is_empty() => { let id = live[(x as usize >> 3) % live.len()]; g.update_node(id, props); }
            4 if !live.is_empty() => { let at = (x as usize >> 3) % live.len(); g.delete_node(live.swap_remove(at)); }
            _ => { let k = (x >> 20) as usize % 4; g.sync_uniques(&pairs[..k.min(3)]); if x.is_multiple_of(2) { g.sync_uniques(&pairs); } }
        }
        let want: usize = pairs.iter().filter(|(t, f)| {
            let (Some(ts), Some(fs)) = (g.names.get(t), g.names.get(f)) else { return false };
            g.uniques.contains(ts, fs)
        }).map(|(t, f)| g.nodes().filter(|n| n.has_label(t) && n.prop(f).is_some()).count()).sum();
        assert_eq!(g.uniques.entries(), want, "step {step}");
        if step % 50 == 0 {
            for (t, f) in &pairs {
                for n in g.nodes() {
                    if let Some(v) = n.prop(f) {
                        if n.has_label(t) {
                            assert!(g.unique_matches(t, f, v).contains(&n.id));
                        }
                    }
                }
            }
        }
    }
}

/// The last two chunks' worth of ids below `u64::MAX`, then one low id:
/// the top ids arrive before anything dense, and a rebuild must place them.
fn top_ids() -> Vec<u64> {
    let mut ids: Vec<u64> = ((u64::MAX - 2048)..(u64::MAX - 1)).collect();
    ids.push(5);
    ids
}

#[test]
fn graph_file_with_ids_in_the_last_chunk_imports() {
    let mut g = Graph::new();
    for id in top_ids() {
        g.restore_node(id, vec!["T".into()], HashMap::new());
    }
    g.reset_next_ids((u64::MAX, 1));
    let mut file = Vec::new();
    crate::graph_file::write(&g, &Default::default(), "test", &mut file).unwrap();
    let (back, _) = crate::graph_file::read(&file[..]).unwrap();
    assert_eq!(back.node_count(), g.node_count());
    let ids: Vec<u64> = back.nodes().map(|n| n.id).collect();
    let mut want = top_ids();
    want.sort_unstable();
    assert_eq!(ids, want);
}
