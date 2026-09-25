//! zega#52: checkpoints, and what a reopen finds after a crash at every step
//! of one.
//!
//! The crash tests run the checkpoint in a child process (this test binary,
//! running `crash_helper`) that aborts at one step, which leaves on disk what
//! a crash or a `kill -9` there would. The parent reopens the directory and
//! compares the whole graph, its id counters, what its last import carried and
//! its declared indexes with an in-memory store that made the same writes and
//! never crashed. A write made after that reopen must survive one more.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use crate::checkpoint::{Step, AT_STEP, CRASH_AT};
use crate::graph::{Graph, Node, NodeId, RelId, Relationship};
use crate::graph_file::{Carried, ExportOptions};
use crate::index::IndexSpec;
use crate::journal::{atomically, Journal};
use crate::location::Point;
use crate::vector::{Metric, Vector};
use crate::wal::{Operation, Wal};
use crate::{Value, Zega, ZegaError};

const CRASH_DIR: &str = "ZEGA_CHECKPOINT_CRASH_DIR";
const CRASH_STEP: &str = "ZEGA_CHECKPOINT_CRASH_AT";

const STEPS: [Step; 11] = [
    Step::GraphPartial,
    Step::GraphWritten,
    Step::GraphSynced,
    Step::GraphRenamed,
    Step::GraphDurable,
    Step::WalNextPartial,
    Step::WalNextWritten,
    Step::WalNextSynced,
    Step::WalRenamed,
    Step::WalDurable,
    Step::ImportsRemoved,
];

const SCHEMA: &str = r#"
    schema {
      type Player { name: String salary: Int favorite -> Team }
      type Team { name: String }
    }
    unique { Player { name } Team { name } }
    index { range Player { salary } }
"#;

/// A disk store with no checkpoint thread: the tests say when.
fn open(path: &Path) -> Zega {
    Zega::open(path.to_str().unwrap())
        .snapshot_every(0)
        .build()
        .unwrap()
}

/// One statement, through the same journal every write path uses.
fn write(zega: &Zega, statement: impl FnOnce(&mut Graph, &mut Journal)) {
    let mut graph = zega.graph.lock().unwrap();
    atomically::<_, ZegaError>(&mut graph, &zega.wal, |graph, journal| {
        statement(graph, journal);
        Ok(())
    })
    .unwrap();
}

fn props(pairs: Vec<(&str, Value)>) -> HashMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// The node whose `key` property is `key`.
fn id_of(graph: &Graph, key: &str) -> NodeId {
    let key = Value::String(key.to_string());
    graph
        .all_nodes()
        .values()
        .find(|node| node.props.get("key") == Some(&key))
        .unwrap_or_else(|| panic!("no node {key:?}"))
        .id
}

fn create(zega: &Zega, key: &str, kind: &[&str], mut extra: Vec<(&str, Value)>) {
    extra.push(("key", Value::String(key.to_string())));
    write(zega, |graph, journal| {
        journal.create_node(graph, labels(kind), props(extra));
    });
}

fn link(zega: &Zega, from: &str, to: &str) {
    write(zega, |graph, journal| {
        let (from, to) = (id_of(graph, from), id_of(graph, to));
        journal.create_relationship(graph, "KNOWS".into(), from, to, props(vec![("since", Value::Int(2026))]));
    });
}

fn set(zega: &Zega, key: &str, value: &str) {
    write(zega, |graph, journal| {
        let id = id_of(graph, key);
        journal.update_node(graph, id, props(vec![("v", Value::String(value.into()))]));
    });
}

fn remove(zega: &Zega, key: &str) {
    write(zega, |graph, journal| {
        let id = id_of(graph, key);
        journal.delete_node(graph, id);
    });
}

/// A `.graph` file with a schema (so uniques and a declared index) and
/// manifest metadata: what an import carries must survive checkpoints too.
fn seed() -> Vec<u8> {
    let zega = Zega::in_memory().build().unwrap();
    create(&zega, "ada", &["Player"], vec![("name", Value::String("Ada".into())), ("salary", Value::Int(300))]);
    create(&zega, "oilers", &["Team"], vec![("name", Value::String("Oilers".into()))]);
    link(&zega, "ada", "oilers");
    let options = ExportOptions {
        schema: Some(SCHEMA.to_string()),
        meta: [("title".to_string(), "seed".to_string())].into(),
    };
    let mut bytes = Vec::new();
    zega.export_with(&mut bytes, &options).unwrap();
    bytes
}

/// Before the first checkpoint: an import, then every kind of value. `a4`,
/// the newest node, is deleted, so only the id counter remembers its id.
fn writes_a(zega: &Zega) {
    zega.import(&seed()[..]).unwrap();
    let point = Value::Point(Point::new(51.05, -114.07).unwrap());
    let vector = Value::Vector(Vector::new(&[0.5, -1.0, 0.0], Metric::Cosine).unwrap());
    let nested = Value::Map(props(vec![
        ("list", Value::List(vec![Value::Int(1), Value::Null, Value::Bool(true)])),
        ("float", Value::Float(1.5f64.to_bits())),
    ]));
    create(zega, "a1", &["Person", "Admin"], vec![("at", point), ("embedding", vector)]);
    create(zega, "a2", &["Person"], vec![("nested", nested)]);
    create(zega, "a3", &["Player"], vec![("name", Value::String("Bo".into())), ("salary", Value::Int(100))]);
    create(zega, "a4", &["Person"], vec![]);
    link(zega, "a1", "a2");
    link(zega, "a2", "a3");
    link(zega, "a3", "a4");
    remove(zega, "a4");
}

/// Between the first checkpoint and the one that crashes.
fn writes_b(zega: &Zega) {
    set(zega, "a1", "b");
    create(zega, "b5", &["Person"], vec![]);
    link(zega, "b5", "a1");
    remove(zega, "a3");
}

/// In the child, acknowledged just before its checkpoint crashes.
fn writes_c(zega: &Zega) {
    set(zega, "a2", "c");
    create(zega, "c6", &["Player"], vec![("name", Value::String("Cy".into())), ("salary", Value::Int(250))]);
    link(zega, "c6", "b5");
}

/// After the reopen that follows the crash.
fn writes_d(zega: &Zega) {
    create(zega, "d7", &["Person"], vec![]);
    link(zega, "d7", "c6");
    set(zega, "a1", "d");
}

#[derive(Debug, PartialEq)]
struct State {
    nodes: HashMap<NodeId, Node>,
    rels: HashMap<RelId, Relationship>,
    next_ids: (NodeId, RelId),
    carried: Carried,
    indexes: Vec<IndexSpec>,
}

/// Everything a reader can observe, and the ids the next writes get.
fn state(zega: &Zega) -> State {
    let graph = zega.graph.lock().unwrap();
    State {
        nodes: graph.all_nodes().clone(),
        rels: graph.all_relationships().clone(),
        next_ids: graph.next_ids(),
        carried: graph.carried().clone(),
        indexes: graph.declared_indexes(),
    }
}

fn reference(writes: &[fn(&Zega)]) -> State {
    let zega = Zega::in_memory().build().unwrap();
    for write in writes {
        write(&zega);
    }
    state(&zega)
}

/// The WAL's entries, read without opening the store.
fn wal_entries(dir: &Path) -> Vec<Operation> {
    Wal::new(&dir.join("wal.bin"), true).unwrap().iter().unwrap()
}

fn wal_version(dir: &Path) -> u16 {
    let bytes = std::fs::read(dir.join("wal.bin")).unwrap();
    u16::from_le_bytes([bytes[4], bytes[5]])
}

/// Every file in `graphs/`, sorted.
fn graph_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir.join("graphs")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// The directory holds exactly what the log reads: one `.graph` file (the
/// one the WAL starts from), no staging file, no half-written next log and
/// no `snapshot.bin`.
fn assert_settled(dir: &Path, context: &str) {
    let entries = wal_entries(dir);
    let Some(Operation::ReplaceGraph { file }) = entries.first() else {
        panic!("{context}: the WAL does not start from a checkpoint: {entries:?}");
    };
    assert_eq!(
        graph_files(dir),
        [file.strip_prefix("graphs/").unwrap().to_string()],
        "{context}: graphs/ holds more than the file the WAL starts from"
    );
    assert!(!dir.join("wal.rotate.tmp").exists(), "{context}: a next log was left behind");
    assert!(!dir.join("snapshot.bin").exists(), "{context}: a superseded snapshot.bin was kept");
}

#[test]
fn a_checkpoint_bounds_the_wal_and_reopens_to_the_same_graph() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    let before = std::fs::metadata(dir.path().join("wal.bin")).unwrap().len();
    let checkpoint = zega.checkpoint().unwrap().unwrap();
    let after = std::fs::metadata(dir.path().join("wal.bin")).unwrap().len();
    assert_eq!((checkpoint.wal_bytes_before, checkpoint.wal_bytes_after), (before, after));
    assert_eq!(state(&zega), reference(&[writes_a]), "a checkpoint changed the graph in memory");
    drop(zega);
    assert_eq!(wal_entries(dir.path()).len(), 1, "the WAL is only the checkpoint's entry");
    assert_eq!(wal_version(dir.path()), 3);
    assert_settled(dir.path(), "after a checkpoint");

    let zega = open(dir.path());
    assert_eq!(state(&zega), reference(&[writes_a]));
    writes_b(&zega);
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(state(&zega), reference(&[writes_a, writes_b]));
}

#[test]
fn crash_helper() {
    let (Some(dir), Some(step)) = (std::env::var_os(CRASH_DIR), std::env::var_os(CRASH_STEP)) else {
        return;
    };
    let zega = open(Path::new(&dir));
    writes_c(&zega);
    let step: u8 = step.to_str().unwrap().parse().unwrap();
    CRASH_AT.store(step, std::sync::atomic::Ordering::SeqCst);
    zega.checkpoint().unwrap();
    // Reaching here means the crash point was never passed.
    println!("CHECKPOINT FINISHED");
}

/// Run `writes_c` and a checkpoint that aborts at `step` in a child process.
fn crash_checkpoint(dir: &Path, step: Step) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "checkpoint_tests::crash_helper", "--nocapture", "--test-threads=1"])
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

/// A store with writes A, a first checkpoint, then writes B: the crashing
/// checkpoint has an earlier `.graph` file to supersede and a WAL tail.
fn prepared() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    zega.checkpoint().unwrap();
    writes_b(&zega);
    dir
}

fn assert_recovers(dir: &Path, context: &str) {
    let zega = open(dir);
    assert_eq!(
        state(&zega),
        reference(&[writes_a, writes_b, writes_c]),
        "{context}: the reopened store differs from the writes it acknowledged"
    );
    assert_settled(dir, context);
    writes_d(&zega);
    drop(zega);
    let zega = open(dir);
    assert_eq!(
        state(&zega),
        reference(&[writes_a, writes_b, writes_c, writes_d]),
        "{context}: a write made after the reopen was lost or changed"
    );
    // And the store can still checkpoint, and reopen from that.
    zega.checkpoint().unwrap();
    drop(zega);
    assert_eq!(
        state(&open(dir)),
        reference(&[writes_a, writes_b, writes_c, writes_d]),
        "{context}: a checkpoint after the recovery lost a write"
    );
}

#[test]
fn a_crash_at_every_checkpoint_step_opens_to_every_acknowledged_write() {
    for step in STEPS {
        let dir = prepared();
        crash_checkpoint(dir.path(), step);
        assert_recovers(dir.path(), &format!("{step:?}"));
    }
}

/// What a power cut can add to the aborts above: the unsynced files of a
/// checkpoint hold anything, including an unreferenced `.graph` file under
/// its final name if its rename reached the disk and its bytes did not
/// (impossible here, since it is synced first, but opening must not care).
#[test]
fn a_power_cut_leaves_nothing_that_stops_an_open() {
    let dir = prepared();
    {
        let zega = open(dir.path());
        writes_c(&zega);
    }
    let graphs = dir.path().join("graphs");
    std::fs::write(graphs.join(".checkpoint-1-0.tmp"), b"\x89ZGRAPH\ntorn").unwrap();
    std::fs::write(graphs.join(format!("{}.graph", "ab".repeat(32))), b"\x89ZGR").unwrap();
    std::fs::write(dir.path().join("wal.rotate.tmp"), b"ZWAL\x03\x00\x40\x00").unwrap();
    assert_recovers(dir.path(), "power cut");
}

/// Writes wait only while the graph is written out: between that and the
/// WAL rotation they go through, and the rotation carries them over.
#[test]
fn writes_go_through_while_a_checkpoint_syncs_and_are_carried_into_the_new_wal() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    let (reached, wait_for_reached) = mpsc::channel();
    let (resume, wait_for_resume) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let checkpoint = scope.spawn(|| {
            AT_STEP.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |step| {
                    if step == Step::GraphDurable {
                        reached.send(()).unwrap();
                        // Bounded, so a failing assertion ends the test
                        // instead of leaving this checkpoint waiting forever.
                        let _ = wait_for_resume.recv_timeout(Duration::from_secs(10));
                    }
                }));
            });
            zega.checkpoint().unwrap().unwrap()
        });
        wait_for_reached.recv_timeout(Duration::from_secs(30)).unwrap();
        // The checkpoint's graph is on disk and the WAL not yet rotated.
        let (done, wait_for_done) = mpsc::channel();
        let zega = &zega;
        scope.spawn(move || {
            writes_b(zega);
            done.send(()).unwrap();
        });
        // Well inside the 10 s the checkpoint waits for `resume`.
        wait_for_done
            .recv_timeout(Duration::from_secs(5))
            .expect("writes waited for a checkpoint that had released the graph");
        resume.send(()).unwrap();
        let checkpoint = checkpoint.join().unwrap();
        assert!(checkpoint.paused < Duration::from_secs(30));
    });
    drop(zega);
    // The new WAL: the checkpoint, then writes B's four statements.
    let entries = wal_entries(dir.path());
    assert!(matches!(entries[0], Operation::ReplaceGraph { .. }), "{entries:?}");
    assert_eq!(entries.len(), 5, "{entries:?}");
    assert_settled(dir.path(), "after a checkpoint with writes during it");
    assert_eq!(state(&open(dir.path())), reference(&[writes_a, writes_b]));
}

/// Many writers and repeated checkpoints at once: every acknowledged write is
/// there after a reopen.
#[test]
fn concurrent_writers_lose_nothing_across_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    let writers = 4;
    let each = 150;
    std::thread::scope(|scope| {
        for writer in 0..writers {
            let zega = &zega;
            scope.spawn(move || {
                for i in 0..each {
                    create(zega, &format!("w{writer}-{i}"), &["Person"], vec![("pad", Value::String("x".repeat(64)))]);
                }
            });
        }
        let zega = &zega;
        scope.spawn(move || {
            for _ in 0..20 {
                zega.checkpoint().unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
        });
    });
    let expected = state(&zega);
    assert_eq!(expected.nodes.len(), writers * each);
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(state(&zega), expected);
}

#[test]
fn a_disk_store_checkpoints_on_its_own_once_the_wal_is_due() {
    let dir = tempfile::tempdir().unwrap();
    let floor = 16 * 1024;
    let zega = Zega::open(dir.path().to_str().unwrap())
        .snapshot_every(floor)
        .build()
        .unwrap();
    for i in 0..200 {
        create(&zega, &format!("n{i}"), &["Person"], vec![("pad", Value::String("x".repeat(200)))]);
    }
    // The WAL, read raw while the store is open: its first entry is a
    // `ReplaceGraph` (variant 6) once a checkpoint has rotated it.
    let starts_from_a_checkpoint = || {
        let bytes = std::fs::read(dir.path().join("wal.bin")).unwrap();
        bytes.len() >= 22 && bytes[18..22] == 6u32.to_le_bytes()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !starts_from_a_checkpoint() {
        assert!(std::time::Instant::now() < deadline, "no checkpoint within 20 s of the WAL passing {floor} bytes");
        std::thread::sleep(Duration::from_millis(20));
    }
    let expected = state(&zega);
    drop(zega);
    let entries = wal_entries(dir.path());
    assert!(matches!(entries[0], Operation::ReplaceGraph { .. }));
    assert!(entries.len() < 200, "the WAL still holds every write: {}", entries.len());
    assert_eq!(state(&open(dir.path())), expected);
}

#[test]
fn snapshot_every_zero_takes_no_checkpoint_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    for i in 0..100 {
        create(&zega, &format!("n{i}"), &["Person"], vec![("pad", Value::String("x".repeat(200)))]);
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(graph_files(dir.path()).is_empty());
    assert_eq!(wal_entries(dir.path()).len(), 100);
}

/// A graph the `.graph` writer refuses cannot be checkpointed: the attempt
/// fails and changes nothing, and the WAL keeps every write as before.
#[test]
fn a_failed_checkpoint_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    zega.checkpoint().unwrap();
    writes_b(&zega);
    write(&zega, |graph, journal| {
        journal.create_node(graph, labels(&["Twice", "Twice"]), HashMap::new());
    });
    let wal = std::fs::read(dir.path().join("wal.bin")).unwrap();
    let files = graph_files(dir.path());
    let error = zega.checkpoint().unwrap_err().to_string();
    assert!(error.contains("twice"), "{error}");
    assert_eq!(std::fs::read(dir.path().join("wal.bin")).unwrap(), wal);
    assert_eq!(graph_files(dir.path()), files, "the failed checkpoint left a file");
    let expected = state(&zega);
    writes_c(&zega);
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(state(&zega).nodes.len(), expected.nodes.len() + 1);
}

/// A store from before checkpoints: a version 2 WAL with every write, and a
/// `snapshot.bin` that `Zega::snapshot` wrote without truncating it.
#[test]
fn a_version_2_wal_with_an_old_snapshot_opens_and_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_b_without_import(&zega);
    crate::wal::snapshot(&zega.graph.lock().unwrap(), &dir.path().join("snapshot.bin")).unwrap();
    create(&zega, "late", &["Person"], vec![]);
    drop(zega);
    assert_eq!(wal_version(dir.path()), 2);
    let expected = {
        let zega = Zega::in_memory().build().unwrap();
        writes_b_without_import(&zega);
        create(&zega, "late", &["Person"], vec![]);
        state(&zega)
    };

    let zega = open(dir.path());
    assert_eq!(state(&zega), expected, "an old store opened differently");
    zega.checkpoint().unwrap();
    drop(zega);
    assert_settled(dir.path(), "old store");
    assert_eq!(wal_version(dir.path()), 3);
    assert_eq!(state(&open(dir.path())), expected);
}

fn writes_b_without_import(zega: &Zega) {
    create(zega, "a1", &["Person"], vec![("n", Value::Int(1))]);
    create(zega, "a2", &["Person"], vec![]);
    link(zega, "a1", "a2");
    set(zega, "a1", "v");
    create(zega, "gone", &["Person"], vec![]);
    remove(zega, "gone");
}

/// A version 3 WAL (an import, zega#105) opens, and a checkpoint deletes the
/// imported file it supersedes.
#[test]
fn a_checkpoint_supersedes_an_import() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    let imported = graph_files(dir.path());
    assert_eq!(imported.len(), 1);
    drop(zega);
    let zega = open(dir.path());
    assert_eq!(state(&zega), reference(&[writes_a]));
    zega.checkpoint().unwrap();
    drop(zega);
    assert_settled(dir.path(), "after the import");
    assert_ne!(graph_files(dir.path()), imported);
    assert_eq!(state(&open(dir.path())), reference(&[writes_a]));
}

/// An import during a checkpoint's sync would be deleted by it: they take
/// turns, and whichever comes last is what the store holds.
#[test]
fn an_import_waits_for_a_checkpoint_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    writes_a(&zega);
    let (reached, wait_for_reached) = mpsc::channel();
    let (resume, wait_for_resume) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let zega = &zega;
        let checkpoint = scope.spawn(move || {
            AT_STEP.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |step| {
                    if step == Step::GraphDurable {
                        reached.send(()).unwrap();
                        // Bounded, so a failing assertion ends the test
                        // instead of leaving this checkpoint waiting forever.
                        let _ = wait_for_resume.recv_timeout(Duration::from_secs(10));
                    }
                }));
            });
            zega.checkpoint().unwrap();
        });
        wait_for_reached.recv_timeout(Duration::from_secs(30)).unwrap();
        let (done, wait_for_done) = mpsc::channel();
        let import = scope.spawn(move || {
            zega.import(&seed()[..]).unwrap();
            done.send(()).unwrap();
        });
        assert!(
            wait_for_done.recv_timeout(Duration::from_millis(300)).is_err(),
            "an import ran during a checkpoint"
        );
        resume.send(()).unwrap();
        checkpoint.join().unwrap();
        import.join().unwrap();
    });
    let imported = {
        let zega = Zega::in_memory().build().unwrap();
        zega.import(&seed()[..]).unwrap();
        state(&zega)
    };
    assert_eq!(state(&zega).nodes, imported.nodes);
    drop(zega);
    assert_eq!(state(&open(dir.path())).nodes, imported.nodes);
    assert_eq!(graph_files(dir.path()).len(), 1);
}
