//! zega#52: checkpoints (snapshot, then empty the WAL) and what a restart
//! finds after a crash at every step of one.
//!
//! The crash tests run the checkpoint in a child process (this test binary,
//! running `crash_helper`) that aborts at one step. The parent then reopens
//! the directory and compares everything a reader can observe, plus the ids
//! the next writes get, with an in-memory store that ran the same writes and
//! never crashed. A write made after that reopen must survive one more.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value as Json;
use tempfile::tempdir;

use crate::wal::tests::switchable_wal;
use crate::wal::{CrashPoint, Operation, Wal, CRASH_AT};
use crate::{Value, Zega};

const CRASH_DIR: &str = "ZEGA_CHECKPOINT_CRASH_DIR";
const CRASH_STEP: &str = "ZEGA_CHECKPOINT_CRASH_AT";

const STEPS: [CrashPoint; 7] = [
    CrashPoint::SnapshotPartial,
    CrashPoint::SnapshotWritten,
    CrashPoint::SnapshotSynced,
    CrashPoint::SnapshotRenamed,
    CrashPoint::SnapshotDurable,
    CrashPoint::WalTruncated,
    CrashPoint::CheckpointWritten,
];

fn run(zega: &Zega, query: &str, params: &[(&str, &str)]) {
    let params = params
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect::<HashMap<_, _>>();
    zega.query(query, params).unwrap();
}

fn person(zega: &Zega, id: &str) {
    run(zega, "CREATE (n:Person {id: $id})", &[("id", id)]);
}

fn knows(zega: &Zega, from: &str, to: &str) {
    run(
        zega,
        "MATCH (a {id: $a}), (b {id: $b}) CREATE (a)-[:KNOWS]->(b)",
        &[("a", from), ("b", to)],
    );
}

fn set_v(zega: &Zega, id: &str, v: &str) {
    run(
        zega,
        "MATCH (n:Person {id: $id}) SET n.v = $v",
        &[("id", id), ("v", v)],
    );
}

fn remove(zega: &Zega, id: &str) {
    run(zega, "MATCH (n {id: $id}) DETACH DELETE n", &[("id", id)]);
}

/// Before the first checkpoint. Deleting u4, the newest node, and its
/// relationship leaves ids that only the counters remember.
fn writes_a(zega: &Zega) {
    for id in ["u1", "u2", "u3", "u4"] {
        person(zega, id);
    }
    knows(zega, "u1", "u2");
    knows(zega, "u2", "u3");
    knows(zega, "u3", "u4");
    remove(zega, "u4");
}

/// Between the first checkpoint and the one that crashes.
fn writes_b(zega: &Zega) {
    set_v(zega, "u1", "b");
    person(zega, "u5");
    knows(zega, "u5", "u1");
    remove(zega, "u3");
}

/// In the child, acknowledged just before its checkpoint crashes.
fn writes_c(zega: &Zega) {
    set_v(zega, "u2", "c");
    person(zega, "u6");
    knows(zega, "u6", "u5");
}

/// After the reopen that follows the crash.
fn writes_d(zega: &Zega) {
    person(zega, "u7");
    knows(zega, "u7", "u6");
    set_v(zega, "u1", "d");
}

/// Everything a reader can observe, plus the ids the next writes get. Nodes
/// and relationships are sorted: `graph_json` lists them in hash-map order.
fn state(zega: &Zega) -> (Json, (u64, u64)) {
    let ids = zega.graph.lock().unwrap().next_ids();
    let mut graph = zega.graph_json().unwrap();
    for key in ["nodes", "rels"] {
        graph[key]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|item| item.to_string());
    }
    (graph, ids)
}

fn open(path: &Path) -> Zega {
    Zega::open(path.to_str().unwrap())
        .checkpoint_after(None)
        .build()
        .unwrap()
}

fn reference(writes: &[fn(&Zega)]) -> (Json, (u64, u64)) {
    let zega = Zega::in_memory().build().unwrap();
    for write in writes {
        write(&zega);
    }
    state(&zega)
}

/// The generation in the log's first entry, 0 without one.
fn wal_generation(path: &Path) -> u64 {
    let wal = Wal::new(&path.join("wal.bin"), true).unwrap();
    match wal.iter().unwrap().first() {
        Some(Operation::Checkpoint { generation }) => *generation,
        _ => 0,
    }
}

#[test]
fn crash_helper() {
    let (Some(dir), Some(step)) = (std::env::var_os(CRASH_DIR), std::env::var_os(CRASH_STEP))
    else {
        return;
    };
    let zega = open(Path::new(&dir));
    writes_c(&zega);
    let step: u8 = step.to_str().unwrap().parse().unwrap();
    CRASH_AT.store(step, std::sync::atomic::Ordering::SeqCst);
    zega.snapshot().unwrap();
    // Reaching here means the crash point was never passed.
    println!("CHECKPOINT FINISHED");
}

/// Run `writes_c` and a checkpoint that aborts at `step` in a child process.
fn crash_checkpoint(dir: &Path, step: CrashPoint) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "checkpoint_tests::crash_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_DIR, dir)
        .env(CRASH_STEP, (step as u8).to_string())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success() && !stdout.contains("CHECKPOINT FINISHED"),
        "{step:?}: the child should have aborted there\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A store with writes A, checkpoint generation 1, then writes B.
fn prepared() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    zega.snapshot().unwrap();
    writes_b(&zega);
    dir
}

fn assert_recovers(dir: &Path, step: &str) {
    let zega = open(dir);
    assert_eq!(
        state(&zega),
        reference(&[writes_a, writes_b, writes_c]),
        "{step}: reopened store differs from the writes it acknowledged"
    );
    writes_d(&zega);
    drop(zega);
    let zega = open(dir);
    assert_eq!(
        state(&zega),
        reference(&[writes_a, writes_b, writes_c, writes_d]),
        "{step}: a write made after the reopen was lost or changed"
    );
}

#[test]
fn a_crash_at_every_checkpoint_step_reopens_to_every_acknowledged_write() {
    for step in STEPS {
        let dir = prepared();
        crash_checkpoint(dir.path(), step);
        assert_recovers(dir.path(), &format!("{step:?}"));
    }
}

/// A power cut can lose what a process crash keeps: the checkpoint entry may
/// be half on disk. Replay drops the torn entry, leaving a header-only log.
#[test]
fn a_torn_checkpoint_entry_reopens_to_every_acknowledged_write() {
    let dir = prepared();
    crash_checkpoint(dir.path(), CrashPoint::CheckpointWritten);
    let wal = dir.path().join("wal.bin");
    let len = std::fs::metadata(&wal).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&wal)
        .unwrap()
        .set_len(len - 3)
        .unwrap();
    assert_recovers(dir.path(), "torn checkpoint entry");
}

#[test]
fn a_checkpoint_empties_the_log_and_restart_reads_the_snapshot() {
    let dir = tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    writes_b(&zega);
    let before = std::fs::metadata(dir.path().join("wal.bin")).unwrap().len();
    zega.snapshot().unwrap();
    let after = std::fs::metadata(dir.path().join("wal.bin")).unwrap().len();
    assert!(
        after < 64,
        "log is {after} bytes after the checkpoint (was {before})"
    );
    assert_eq!(zega.wal.len(), after);
    assert_eq!(wal_generation(dir.path()), 1);
    drop(zega);
    assert_eq!(state(&open(dir.path())), reference(&[writes_a, writes_b]));
}

#[test]
fn ids_of_deleted_nodes_are_not_handed_out_again_after_a_checkpoint() {
    let dir = tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega); // u4, the newest node, and its relationship are deleted
    let ids = zega.graph.lock().unwrap().next_ids();
    zega.snapshot().unwrap();
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(zega.graph.lock().unwrap().next_ids(), ids);
}

#[test]
fn a_log_newer_than_the_snapshot_refuses_to_open() {
    let dir = tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    zega.snapshot().unwrap();
    writes_b(&zega);
    drop(zega);
    std::fs::remove_file(dir.path().join("snapshot.bin")).unwrap();
    let error = Zega::open(dir.path().to_str().unwrap())
        .build()
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("snapshot.bin is missing or older than the log"),
        "{error}"
    );
}

#[test]
fn a_snapshot_from_before_generations_still_loads_under_its_log() {
    // What `Zega::snapshot` wrote before checkpoints: the header-less bytes,
    // with the whole log left in place and replayed over it.
    let dir = tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    std::fs::write(
        dir.path().join("snapshot.bin"),
        zega.snapshot_bytes().unwrap(),
    )
    .unwrap();
    writes_b(&zega);
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(state(&zega), reference(&[writes_a, writes_b]));
    zega.snapshot().unwrap();
    drop(zega);
    assert_eq!(wal_generation(dir.path()), 1);
    assert_eq!(state(&open(dir.path())), reference(&[writes_a, writes_b]));
}

#[test]
fn writes_checkpoint_on_their_own_once_the_log_passes_the_threshold() {
    let dir = tempdir().unwrap();
    let min = 4096;
    let zega = Zega::open(dir.path().to_str().unwrap())
        .checkpoint_after(Some(min))
        .build()
        .unwrap();
    let snapshot_len =
        || std::fs::metadata(dir.path().join("snapshot.bin")).map_or(0, |meta| meta.len());
    for i in 0..800 {
        person(&zega, &format!("n{i}"));
        // The log never outgrows the larger of the minimum and the snapshot
        // by more than the one write that crossed it.
        let bound = snapshot_len().max(min) + 512;
        assert!(
            zega.wal.len() < bound,
            "log is {} bytes at write {i}",
            zega.wal.len()
        );
    }
    assert!(snapshot_len() > min, "snapshot is {} bytes", snapshot_len());
    let generation = zega.generation.load(std::sync::atomic::Ordering::SeqCst);
    assert!(generation > 1, "checkpointed {generation} times");
    let expected = state(&zega);
    drop(zega);
    assert_eq!(state(&open(dir.path())), expected);
}

#[test]
fn a_failed_log_rewrite_after_the_snapshot_refuses_writes_until_reopen() {
    let dir = tempdir().unwrap();
    let mut zega = open(dir.path());
    writes_a(&zega);
    let (wal, switch) = switchable_wal(&dir.path().join("wal.bin"));
    zega.wal = wal;
    writes_b(&zega);
    switch.refuse(true);
    let error = zega.snapshot().unwrap_err().to_string();
    assert!(error.contains("WAL checkpoint failed"), "{error}");
    switch.refuse(false);
    // A write now would land in a log the new snapshot already covers and be
    // skipped on replay, so it must be refused, not lost.
    let refused = zega.query("CREATE (n:Person {id: 'lost'})", HashMap::new());
    assert!(
        refused.is_err(),
        "a write after a failed checkpoint was accepted"
    );
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(state(&zega), reference(&[writes_a, writes_b]));
    writes_c(&zega);
    drop(zega);
    assert_eq!(
        state(&open(dir.path())),
        reference(&[writes_a, writes_b, writes_c])
    );
}
