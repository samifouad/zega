//! zega#51: a statement releases the graph lock before it waits for its WAL
//! entry to be durable, so other statements run during an fsync and waiting
//! writers share one. What must still hold: nobody is answered with a write
//! that is not durable, a writer only once its own entry is, and after a
//! failed sync nothing is read from a graph that holds writes the log lost.
//!
//! The WAL here syncs through a gate the test holds, releases or fails.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use crate::wal::tests::{gated_wal, SyncGate};
use crate::{Value, Zega};

struct Store {
    // First: fields drop in order, and this must run before the WAL does.
    _release: ReleaseOnDrop,
    zega: Arc<Zega>,
    gate: Arc<SyncGate>,
    dir: TempDir,
}

/// A test that fails with the gate held would otherwise hang: dropping the
/// store joins a WAL worker waiting at the gate.
struct ReleaseOnDrop(Arc<SyncGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release(false);
    }
}

impl Store {
    fn open() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut zega = Zega::open(dir.path().to_str().unwrap()).build().unwrap();
        let (wal, gate) = gated_wal(&dir.path().join("wal.bin"));
        zega.wal = wal;
        let release = ReleaseOnDrop(Arc::clone(&gate));
        Store { zega: Arc::new(zega), gate, dir, _release: release }
    }

    fn reopen(self) -> Zega {
        let path = self.dir.path().to_str().unwrap().to_string();
        self.gate.release(false);
        drop(Arc::try_unwrap(self.zega).ok().expect("store still shared"));
        Zega::open(&path).build().unwrap()
    }
}

fn create(zega: &Zega, name: &str) -> crate::Result<()> {
    zega.query(
        "CREATE (n:Person {name: $name})",
        HashMap::from([("name".to_string(), Value::String(name.to_string()))]),
    )
    .map(|_| ())
}

fn count(zega: &Zega) -> crate::Result<i64> {
    let rows = zega.query("MATCH (n:Person) RETURN count(n) AS c", HashMap::new())?;
    match rows[0].fields.get("c") {
        Some(Value::Int(count)) => Ok(*count),
        other => panic!("count was {other:?}"),
    }
}

/// Run `f` on its own thread; the receiver gets its result.
fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(f());
    });
    receive
}

fn still_waiting<T>(receiver: &mpsc::Receiver<T>) -> bool {
    matches!(
        receiver.recv_timeout(Duration::from_millis(200)),
        Err(mpsc::RecvTimeoutError::Timeout)
    )
}

#[test]
fn a_write_is_answered_only_once_its_entry_is_synced() {
    let store = Store::open();
    store.gate.hold();
    let zega = Arc::clone(&store.zega);
    let writer = spawn(move || create(&zega, "ada"));
    store.gate.wait_for_held_sync();
    assert!(still_waiting(&writer), "the write was answered before its sync");
    store.gate.release(false);
    writer.recv().unwrap().unwrap();
    assert_eq!(store.gate.syncs(), 1);
    assert_eq!(count(&store.reopen()).unwrap(), 1);
}

#[test]
fn writers_append_during_a_sync_and_share_the_next_one() {
    let store = Store::open();
    create(&store.zega, "before").unwrap();
    store.gate.hold();
    let zega = Arc::clone(&store.zega);
    let first = spawn(move || create(&zega, "first"));
    store.gate.wait_for_held_sync();

    // The graph lock is free while `first` waits for its sync. A read runs,
    // but it saw `first`'s entry, so it answers only once that is durable.
    let zega = Arc::clone(&store.zega);
    let names = spawn(move || {
        zega.query("MATCH (n:Person {name: 'before'}) RETURN n.name AS name", HashMap::new())
    });
    assert!(still_waiting(&names), "a read returned while a write it saw was not durable");

    // And other writers append meanwhile, joining the next sync.
    let writers: Vec<_> = (0..7)
        .map(|i| {
            let zega = Arc::clone(&store.zega);
            spawn(move || create(&zega, &format!("w{i}")))
        })
        .collect();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while store.zega.wal.appended_sequence() < 9 {
        assert!(
            std::time::Instant::now() < deadline,
            "writers could not append while a sync was in progress ({} appended)",
            store.zega.wal.appended_sequence()
        );
        thread::sleep(Duration::from_millis(5));
    }
    store.gate.release(false);
    first.recv().unwrap().unwrap();
    for writer in writers {
        writer.recv().unwrap().unwrap();
    }
    assert_eq!(names.recv().unwrap().unwrap().len(), 1);
    // `before`'s sync, `first`'s, then one for all seven that queued behind it.
    assert_eq!(store.gate.syncs(), 3, "the seven writers did not share a sync");
    assert_eq!(count(&store.reopen()).unwrap(), 9);
}

#[test]
fn a_read_never_returns_a_write_whose_sync_then_fails() {
    let store = Store::open();
    create(&store.zega, "durable").unwrap();
    store.gate.hold();
    let zega = Arc::clone(&store.zega);
    let writer = spawn(move || create(&zega, "lost"));
    store.gate.wait_for_held_sync();

    // The reader runs under the lock the writer released, sees the write,
    // and must not answer with it before it is durable.
    let zega = Arc::clone(&store.zega);
    let reader = spawn(move || count(&zega));
    assert!(still_waiting(&reader), "a read returned a write that was not durable");

    store.gate.release(true);
    let written = writer.recv().unwrap();
    let read = reader.recv().unwrap();
    assert!(written.is_err(), "the write whose sync failed was acknowledged");
    assert!(read.is_err(), "a read returned {read:?}, which includes a write that was lost");

    // The graph in memory still holds the lost write: nothing more is read
    // from it, and nothing written to it, until the store is reopened.
    let error = count(&store.zega).unwrap_err().to_string();
    assert!(error.contains("refuses reads and writes until it is reopened"), "{error}");
    assert!(create(&store.zega, "after").is_err());

    // The reopened store has exactly the durable write.
    assert_eq!(count(&store.reopen()).unwrap(), 1);
}

#[test]
fn a_read_with_nothing_pending_does_not_wait_for_a_sync() {
    let store = Store::open();
    create(&store.zega, "ada").unwrap();
    store.gate.hold();
    // Nothing appended since the last sync: no wait, though the gate is held.
    assert_eq!(count(&store.zega).unwrap(), 1);
    store.gate.release(false);
}
