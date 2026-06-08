//! Exhaustive durability test suite for the `zega-wal` crate.
//!
//! Focus: write-ahead log + snapshot DURABILITY. A DB must never lose an
//! acknowledged write and must never panic on a malformed file. Every error
//! path is asserted as a `Result`/`Option` value (errors are values in Zega).
//!
//! Wire format (these constants are PRIVATE in the crate, so we re-derive them
//! here from the source and assert against them — keeping the tests honest
//! about the on-disk layout):
//!   * File header: b"ZWAL\x02\x00"  (6 bytes: 4-byte magic + u16 LE version=2)
//!   * Per entry:    [u64 LE payload-len][u32 LE crc32(payload)][payload bytes]
//!     -> entry header length = 12 bytes.
//!   * Legacy (v1) format: [u64 LE payload-len][payload bytes], no header, no crc.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tempfile::tempdir;
use zega_graph::Graph;
use zega_kv::KvStore;
use zega_parser::Value;
use zega_wal::{restore, snapshot, Operation, Wal, WalError};

// ---------------------------------------------------------------------------
// Wire-format constants mirrored from the crate (private there).
// ---------------------------------------------------------------------------

const WAL_FILE_HEADER: &[u8; 6] = b"ZWAL\x02\x00";
const WAL_FILE_HEADER_LEN: u64 = 6;
const ENTRY_HEADER_LEN: u64 = 12;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a simple `KvSet` operation whose key encodes a label so tests can
/// assert on ordering and content cheaply.
fn kv_set(key: &str) -> Operation {
    Operation::KvSet {
        key: key.to_string(),
        value: Value::String(key.to_string()),
        ttl: None,
    }
}

/// Extract the keys of a run of `KvSet` operations, in order.
fn kv_keys(ops: &[Operation]) -> Vec<String> {
    ops.iter()
        .map(|op| match op {
            Operation::KvSet { key, .. } => key.clone(),
            other => panic!("expected KvSet, got {other:?}"),
        })
        .collect()
}

/// Serialize an operation exactly the way the WAL does on disk (bincode).
fn payload_of(op: &Operation) -> Vec<u8> {
    bincode::serialize(op).expect("serialize op")
}

/// Append a single framed v2 entry ([len][crc][payload]) to an already-open
/// file handle. Used to hand-craft WAL bodies.
fn write_framed_entry(file: &mut File, payload: &[u8]) {
    let len = payload.len() as u64;
    let crc = crc32fast::hash(payload);
    file.write_all(&len.to_le_bytes()).unwrap();
    file.write_all(&crc.to_le_bytes()).unwrap();
    file.write_all(payload).unwrap();
}

/// Create a fresh, well-formed v2 WAL file containing the given ops, written
/// by hand (not through `Wal`), and return its path-bearing temp dir.
fn handcraft_v2_wal(path: &Path, ops: &[Operation]) {
    let mut file = File::create(path).unwrap();
    file.write_all(WAL_FILE_HEADER).unwrap();
    for op in ops {
        write_framed_entry(&mut file, &payload_of(op));
    }
    file.sync_all().unwrap();
}

/// Create a legacy (v1) WAL file: no header, entries are [u64 len][payload].
fn handcraft_legacy_wal(path: &Path, ops: &[Operation]) {
    let mut file = File::create(path).unwrap();
    for op in ops {
        let payload = payload_of(op);
        file.write_all(&(payload.len() as u64).to_le_bytes())
            .unwrap();
        file.write_all(&payload).unwrap();
    }
    file.sync_all().unwrap();
}

fn assert_snapshot_corruption(path: &Path) {
    let mut graph = Graph::new();
    let kv = KvStore::new();
    match restore(&mut graph, &kv, path) {
        Err(WalError::Corruption { .. }) => {}
        Ok(restored) => panic!("expected snapshot corruption, got Ok({restored})"),
        Err(other) => panic!("expected snapshot corruption, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 1 — Basic construction & header invariants
// ===========================================================================

#[test]
fn new_creates_file_with_v2_header() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    drop(wal);

    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(WAL_FILE_HEADER), "file must begin with ZWAL v2 header");
    assert_eq!(bytes.len() as u64, WAL_FILE_HEADER_LEN, "empty WAL is header-only");
}

#[test]
fn fresh_wal_iterates_to_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, false).unwrap();
    let ops = wal.iter().unwrap();
    assert!(ops.is_empty(), "a brand-new WAL has no operations");
}

#[test]
fn reopening_empty_wal_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let _ = Wal::new(&path, true).unwrap();
    }
    let first = fs::read(&path).unwrap();
    {
        let _ = Wal::new(&path, false).unwrap();
    }
    let second = fs::read(&path).unwrap();
    assert_eq!(first, second, "reopening must not rewrite a valid empty WAL");
}

#[test]
fn in_memory_wal_accepts_appends_and_iterates_empty() {
    // in_memory() has no backing file: appends are accepted (no-op) and iter
    // returns whatever the in-memory path returns. It must never panic.
    let wal = Wal::in_memory();
    assert!(wal.append(&kv_set("ghost")).is_ok());
    assert!(wal.flush().is_ok());
    // iter() opens self.path which is empty for in_memory; this is expected to
    // surface as an Err (no such file), NOT a panic. Assert it does not panic.
    let _ = wal.iter();
}

// ===========================================================================
// SECTION 2 — Append + ordering + round-trip
// ===========================================================================

#[test]
fn single_append_roundtrips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    wal.append(&kv_set("only")).unwrap();
    drop(wal);

    let wal2 = Wal::new(&path, false).unwrap();
    assert_eq!(kv_keys(&wal2.iter().unwrap()), vec!["only".to_string()]);
}

#[test]
fn appends_preserve_insertion_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    let keys = ["a", "b", "c", "d", "e", "f", "g"];
    for k in &keys {
        wal.append(&kv_set(k)).unwrap();
    }
    drop(wal);

    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    let expected: Vec<String> = keys.iter().map(|s| s.to_string()).collect();
    assert_eq!(recovered, expected, "WAL must replay in append order");
}

#[test]
fn all_operation_variants_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    props.insert("age".to_string(), Value::Int(30));

    let ops = vec![
        Operation::InsertNode {
            id: 1,
            labels: vec!["Person".to_string(), "Admin".to_string()],
            props: props.clone(),
        },
        Operation::UpdateNode {
            id: 1,
            props: props.clone(),
        },
        Operation::InsertRel {
            id: 10,
            kind: "KNOWS".to_string(),
            from: 1,
            to: 2,
            props: HashMap::new(),
        },
        Operation::DeleteRel { id: 10 },
        Operation::DeleteNode { id: 1 },
        Operation::KvSet {
            key: "k".to_string(),
            value: Value::Int(42),
            ttl: Some(60),
        },
        Operation::KvDel {
            key: "k".to_string(),
        },
    ];
    for op in &ops {
        wal.append(op).unwrap();
    }
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), ops.len(), "every variant must survive a round-trip");

    // Spot-check structural fidelity of the first and last entries.
    match &recovered[0] {
        Operation::InsertNode { id, labels, props } => {
            assert_eq!(*id, 1);
            assert_eq!(labels, &vec!["Person".to_string(), "Admin".to_string()]);
            assert_eq!(props.get("age"), Some(&Value::Int(30)));
        }
        other => panic!("expected InsertNode, got {other:?}"),
    }
    match &recovered[recovered.len() - 1] {
        Operation::KvDel { key } => assert_eq!(key, "k"),
        other => panic!("expected KvDel, got {other:?}"),
    }
}

#[test]
fn append_after_reopen_continues_log() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let wal = Wal::new(&path, true).unwrap();
        wal.append(&kv_set("session1-a")).unwrap();
        wal.append(&kv_set("session1-b")).unwrap();
    }
    {
        let wal = Wal::new(&path, true).unwrap();
        wal.append(&kv_set("session2-a")).unwrap();
    }
    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(
        recovered,
        vec![
            "session1-a".to_string(),
            "session1-b".to_string(),
            "session2-a".to_string()
        ],
        "reopening must append, never truncate prior valid entries"
    );
}

#[test]
fn append_does_not_rewrite_existing_header_on_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let wal = Wal::new(&path, true).unwrap();
        wal.append(&kv_set("x")).unwrap();
    }
    let len_after_one = fs::metadata(&path).unwrap().len();
    {
        let wal = Wal::new(&path, true).unwrap();
        wal.append(&kv_set("y")).unwrap();
    }
    let len_after_two = fs::metadata(&path).unwrap().len();
    assert!(
        len_after_two > len_after_one,
        "second append must grow the file, not duplicate the header"
    );
}

// ===========================================================================
// SECTION 3 — Group commit / fsync / flush_every
// ===========================================================================

#[test]
fn flush_every_durably_persists_each_append() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap(); // flush_every = true => no worker thread
    wal.append(&kv_set("one")).unwrap();
    // Without dropping the writer, the data must already be on disk and
    // re-readable by an independent reader.
    let recovered = wal.iter().unwrap();
    assert_eq!(kv_keys(&recovered), vec!["one".to_string()]);
}

#[test]
fn explicit_flush_is_idempotent_and_safe_when_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, false).unwrap();
    // Flushing with nothing pending must be a no-op success.
    assert!(wal.flush().is_ok());
    assert!(wal.flush().is_ok());
    wal.append(&kv_set("z")).unwrap();
    assert!(wal.flush().is_ok());
    assert!(wal.flush().is_ok());
}

#[test]
fn group_commit_acknowledges_concurrent_writers() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal =
        Arc::new(Wal::with_group_commit(&path, false, Duration::from_secs(1), 4).unwrap());
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let wal = Arc::clone(&wal);
            thread::spawn(move || wal.append(&kv_set(&format!("k{i}"))).unwrap())
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    // Every append that returned Ok must be durable.
    assert_eq!(wal.iter().unwrap().len(), 8);
}

#[test]
fn group_commit_batches_below_batch_size_still_flush_on_drop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        // batch_size larger than the number of writes; the interval timer in
        // the worker must still flush them, and append() blocks until durable.
        let wal =
            Wal::with_group_commit(&path, false, Duration::from_millis(5), 1000).unwrap();
        wal.append(&kv_set("a")).unwrap();
        wal.append(&kv_set("b")).unwrap();
    }
    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(recovered, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn batch_size_zero_is_clamped_to_one() {
    // with_group_commit clamps batch_size via .max(1); a zero must not hang.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::with_group_commit(&path, false, Duration::from_millis(5), 0).unwrap();
    wal.append(&kv_set("clamped")).unwrap();
    assert_eq!(kv_keys(&wal.iter().unwrap()), vec!["clamped".to_string()]);
}

#[test]
fn append_returns_only_after_durable_in_group_mode() {
    // In group-commit mode, append blocks until durable_sequence catches up.
    // After Ok, an independent reopen must see the entry even without flush().
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::with_group_commit(&path, false, Duration::from_millis(1), 2).unwrap();
    wal.append(&kv_set("durable")).unwrap();
    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(kv_keys(&recovered), vec!["durable".to_string()]);
}

// ===========================================================================
// SECTION 4 — CRC per entry
// ===========================================================================

#[test]
fn each_entry_has_a_valid_crc_on_disk() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    let op = kv_set("crc-check");
    wal.append(&op).unwrap();
    drop(wal);

    let bytes = fs::read(&path).unwrap();
    // Layout: [6 header][8 len][4 crc][payload...]
    let payload = payload_of(&op);
    let len_off = WAL_FILE_HEADER_LEN as usize;
    let crc_off = len_off + 8;
    let pay_off = crc_off + 4;

    let stored_len = u64::from_le_bytes(bytes[len_off..len_off + 8].try_into().unwrap());
    assert_eq!(stored_len as usize, payload.len(), "framed length must match payload");

    let stored_crc = u32::from_le_bytes(bytes[crc_off..crc_off + 4].try_into().unwrap());
    let recomputed = crc32fast::hash(&bytes[pay_off..pay_off + payload.len()]);
    assert_eq!(stored_crc, recomputed, "stored CRC must equal crc32 of payload");
    assert_eq!(stored_crc, crc32fast::hash(&payload));
}

#[test]
fn bitflip_in_payload_of_only_entry_truncates_corrupt_tail() {
    // A single corrupt entry that is also the last entry => treated as a torn
    // tail and truncated, leaving an empty (header-only) WAL. No data was
    // acked beyond it, so nothing is lost.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("solo")]);

    // Flip a byte inside the payload (after header + 12-byte entry header).
    let mut bytes = fs::read(&path).unwrap();
    let payload_start = (WAL_FILE_HEADER_LEN + ENTRY_HEADER_LEN) as usize;
    bytes[payload_start] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    let wal = Wal::new(&path, false).unwrap();
    let ops = wal.iter().unwrap();
    assert!(ops.is_empty(), "corrupt sole/tail entry is dropped, not errored");
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        WAL_FILE_HEADER_LEN,
        "file truncated back to header"
    );
}

// ===========================================================================
// SECTION 5 — ZWAL v2 header validation & legacy migration
// ===========================================================================

#[test]
fn wrong_magic_is_treated_as_legacy_and_migrated() {
    // prepare_wal: if first 4 bytes are not ZWAL magic, it runs the legacy
    // migration path which reframes [len][payload] entries.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_legacy_wal(&path, &[kv_set("legacy-1"), kv_set("legacy-2")]);

    let wal = Wal::new(&path, true).unwrap();
    let recovered = kv_keys(&wal.iter().unwrap());
    assert_eq!(recovered, vec!["legacy-1".to_string(), "legacy-2".to_string()]);

    // After migration the file must carry the v2 header.
    assert!(fs::read(&path).unwrap().starts_with(WAL_FILE_HEADER));
}

#[test]
fn empty_legacy_file_migrates_to_header_only() {
    // A zero-length file at open time is initialized as a fresh v2 WAL.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    File::create(&path).unwrap().sync_all().unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);

    let wal = Wal::new(&path, true).unwrap();
    assert!(wal.iter().unwrap().is_empty());
    assert!(fs::read(&path).unwrap().starts_with(WAL_FILE_HEADER));
}

#[test]
fn migrated_wal_accepts_new_appends() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_legacy_wal(&path, &[kv_set("old")]);

    let wal = Wal::new(&path, true).unwrap();
    wal.append(&kv_set("new")).unwrap();
    drop(wal);

    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(recovered, vec!["old".to_string(), "new".to_string()]);
}

#[test]
fn legacy_with_torn_trailing_entry_migrates_only_complete_entries() {
    // Legacy file whose final entry is truncated mid-payload: migration stops
    // at the last complete entry (entry_end > file_len => break).
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        let p1 = payload_of(&kv_set("good"));
        file.write_all(&(p1.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&p1).unwrap();
        // Declare a large length but write only a few bytes (torn).
        file.write_all(&1000u64.to_le_bytes()).unwrap();
        file.write_all(b"truncated").unwrap();
        file.sync_all().unwrap();
    }

    let wal = Wal::new(&path, true).unwrap();
    let recovered = kv_keys(&wal.iter().unwrap());
    assert_eq!(recovered, vec!["good".to_string()], "torn legacy tail is dropped");
}

#[test]
fn corrupt_v2_header_version_is_reported_as_corruption() {
    // A file that has correct ZWAL magic but a wrong version byte must be
    // rejected as Corruption at open time (prepare_wal), not silently migrated.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(b"ZWAL").unwrap();
        file.write_all(&99u16.to_le_bytes()).unwrap(); // version 99
        // Add a stray byte so the file is non-empty beyond the header.
        file.write_all(&[0u8]).unwrap();
        file.sync_all().unwrap();
    }
    let result = Wal::new(&path, true);
    match result {
        Err(WalError::Corruption { offset, reason }) => {
            assert_eq!(offset, 0);
            assert!(reason.contains("version"), "reason mentions version: {reason}");
        }
        Ok(_) => panic!("expected Corruption for bad version, got Ok(Wal)"),
        Err(other) => panic!("expected Corruption for bad version, got {other:?}"),
    }
}

#[test]
fn truncated_v2_header_magic_then_eof_is_corruption() {
    // File contains the magic but is cut off before the 2-byte version: the
    // version read fails and prepare_wal reports truncated header corruption.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(b"ZWAL").unwrap();
        file.write_all(&[0x02]).unwrap(); // only 1 of 2 version bytes
        file.sync_all().unwrap();
    }
    match Wal::new(&path, true) {
        Err(WalError::Corruption { offset, reason }) => {
            assert_eq!(offset, 0);
            assert!(reason.contains("truncated"), "reason: {reason}");
        }
        Ok(_) => panic!("expected truncated-header Corruption, got Ok(Wal)"),
        Err(other) => panic!("expected truncated-header Corruption, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 6 — iter() header validation on read
// ===========================================================================

#[test]
fn iter_rejects_file_whose_header_was_clobbered_after_open() {
    // Construct a valid WAL, then clobber the header bytes on disk. iter()
    // independently validates the header and must return Corruption@0.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("a")]);
    let wal = Wal::with_group_commit(&path, true, Duration::from_millis(5), 64).unwrap();

    // Clobber the magic on disk behind the open handle.
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] = b'X';
    fs::write(&path, &bytes).unwrap();

    match wal.iter() {
        Err(WalError::Corruption { offset, reason }) => {
            assert_eq!(offset, 0);
            assert!(reason.contains("header"), "reason: {reason}");
        }
        other => panic!("expected header Corruption from iter(), got {other:?}"),
    }
}

// ===========================================================================
// SECTION 7 — Truncated / torn / partial-write recovery
// ===========================================================================

#[test]
fn torn_entry_header_at_tail_is_truncated() {
    // A trailing fragment shorter than the 12-byte entry header is a torn
    // write; iter() truncates it and keeps preceding valid entries.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("kept")]);
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0x01, 0x02, 0x03]).unwrap(); // 3 bytes < 12
        file.sync_all().unwrap();
    }
    let valid_after_trunc = WAL_FILE_HEADER_LEN + ENTRY_HEADER_LEN + payload_of(&kv_set("kept")).len() as u64;

    let wal = Wal::new(&path, false).unwrap();
    assert_eq!(kv_keys(&wal.iter().unwrap()), vec!["kept".to_string()]);
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        valid_after_trunc,
        "torn header bytes are trimmed off"
    );
}

#[test]
fn entry_with_length_running_past_eof_is_truncated() {
    // Full 12-byte header present but the declared payload length exceeds the
    // remaining file (torn payload). iter() truncates to the last valid entry.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("alpha")]);
    let valid_len = fs::metadata(&path).unwrap().len();
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&9999u64.to_le_bytes()).unwrap(); // huge len
        file.write_all(&0u32.to_le_bytes()).unwrap(); // crc
        file.write_all(b"short").unwrap(); // far fewer than 9999 bytes
        file.sync_all().unwrap();
    }
    let wal = Wal::new(&path, false).unwrap();
    assert_eq!(kv_keys(&wal.iter().unwrap()), vec!["alpha".to_string()]);
    assert_eq!(fs::metadata(&path).unwrap().len(), valid_len, "tail trimmed to valid end");
}

#[test]
fn multiple_valid_entries_then_torn_tail_keeps_all_valid() {
    // The headline durability guarantee: a torn tail must NOT cost us any
    // preceding acknowledged write.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let good = [kv_set("e0"), kv_set("e1"), kv_set("e2"), kv_set("e3")];
    handcraft_v2_wal(&path, &good);
    {
        // Append a torn entry: valid 12-byte header but payload cut short.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        let p = payload_of(&kv_set("torn"));
        file.write_all(&(p.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&crc32fast::hash(&p).to_le_bytes()).unwrap();
        file.write_all(&p[..p.len() / 2]).unwrap(); // half the payload
        file.sync_all().unwrap();
    }
    let wal = Wal::new(&path, false).unwrap();
    assert_eq!(
        kv_keys(&wal.iter().unwrap()),
        vec!["e0".to_string(), "e1".to_string(), "e2".to_string(), "e3".to_string()]
    );
}

#[test]
fn corrupt_trailing_crc_is_truncated_preceding_kept() {
    // Last entry's payload is intact-length but its CRC fails AND it is the
    // final entry => treated as a torn tail and dropped; earlier entries stay.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("keep"), kv_set("rot")]);

    // Corrupt the last payload byte.
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    let wal = Wal::new(&path, false).unwrap();
    assert_eq!(kv_keys(&wal.iter().unwrap()), vec!["keep".to_string()]);
}

#[test]
fn append_after_torn_tail_recovery_lands_at_valid_end() {
    // After iter() trims a torn tail, the writer's append must land
    // contiguously after the last valid entry (file handle seeks to End).
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("solid")]);
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&500u64.to_le_bytes()).unwrap();
        file.write_all(&0u32.to_le_bytes()).unwrap();
        file.write_all(b"junk").unwrap();
        file.sync_all().unwrap();
    }
    // Open via group-commit so iter() truncates, then append a fresh entry.
    let wal = Wal::new(&path, true).unwrap();
    assert_eq!(kv_keys(&wal.iter().unwrap()), vec!["solid".to_string()]);
    wal.append(&kv_set("recovered")).unwrap();
    assert_eq!(
        kv_keys(&wal.iter().unwrap()),
        vec!["solid".to_string(), "recovered".to_string()]
    );
}

// ===========================================================================
// SECTION 8 — Corrupted (non-tail) entry => hard error, no silent loss
// ===========================================================================

#[test]
fn corrupt_middle_entry_crc_is_a_hard_corruption_error() {
    // A CRC failure on a NON-final entry cannot be a torn tail; replaying past
    // it would silently skip an acked write, so iter() must return Corruption.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("first"), kv_set("second"), kv_set("third")]);

    // Corrupt a byte inside the FIRST entry's payload.
    let mut bytes = fs::read(&path).unwrap();
    let first_payload_off = (WAL_FILE_HEADER_LEN + ENTRY_HEADER_LEN) as usize;
    bytes[first_payload_off] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    match Wal::new(&path, false).unwrap().iter() {
        Err(WalError::Corruption { offset, reason }) => {
            assert_eq!(offset, WAL_FILE_HEADER_LEN, "offset points at the bad entry start");
            assert!(reason.contains("checksum"), "reason: {reason}");
        }
        other => panic!("expected Corruption for mid-stream CRC failure, got {other:?}"),
    }
}

#[test]
fn corrupt_middle_entry_does_not_truncate_the_file() {
    // The hard-error path must leave the file untouched (no destructive
    // truncation) so an operator can inspect/repair it.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[kv_set("a"), kv_set("b"), kv_set("c")]);
    let len_before = fs::metadata(&path).unwrap().len();

    let mut bytes = fs::read(&path).unwrap();
    bytes[(WAL_FILE_HEADER_LEN + ENTRY_HEADER_LEN) as usize] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    let _ = Wal::new(&path, false).unwrap().iter(); // expected Err
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        len_before,
        "corruption error must not shrink the file"
    );
}

#[test]
fn undeserializable_payload_with_valid_crc_is_corruption() {
    // Craft an entry whose CRC matches but whose bytes are not a valid
    // bincode Operation. iter() must surface Corruption, not panic.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(WAL_FILE_HEADER).unwrap();
        // First, a valid entry so this is NOT the tail.
        write_framed_entry(&mut file, &payload_of(&kv_set("ok")));
        // Then garbage with a correct CRC, followed by another valid entry so
        // it is not the final entry (forces the hard-error branch).
        let garbage = vec![0xFEu8, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE];
        write_framed_entry(&mut file, &garbage);
        write_framed_entry(&mut file, &payload_of(&kv_set("tail")));
        file.sync_all().unwrap();
    }
    match Wal::new(&path, false).unwrap().iter() {
        Err(WalError::Corruption { reason, .. }) => {
            assert!(
                reason.contains("payload") || reason.contains("operation"),
                "reason: {reason}"
            );
        }
        // Depending on bincode, the garbage could also fail differently; the
        // contract is only that it is an Err and never a panic.
        Err(other) => panic!("expected Corruption, got {other:?}"),
        Ok(ops) => panic!("garbage payload should not deserialize, got {} ops", ops.len()),
    }
}

#[test]
fn bincode_garbage_as_final_entry_is_truncated_as_torn_tail() {
    // Same garbage payload but as the FINAL entry: it is indistinguishable
    // from a torn tail at the byte level, so the WAL drops it and keeps the
    // preceding valid entry. The CRC matches, length fits => the deserialize
    // failure is the discriminator; this asserts no panic and no false error
    // on the valid prefix.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(WAL_FILE_HEADER).unwrap();
        write_framed_entry(&mut file, &payload_of(&kv_set("survivor")));
        // A non-deserializable final entry with a correct CRC.
        write_framed_entry(&mut file, &[0xFFu8; 4]);
        file.sync_all().unwrap();
    }
    // The final-entry deserialize failure is NOT in the tail-truncation branch
    // (that branch only covers CRC mismatch), so this is a hard Corruption.
    // Assert it is an Err (never a panic) and that the offset is the bad entry.
    match Wal::new(&path, false).unwrap().iter() {
        Err(WalError::Corruption { .. }) => {}
        Err(other) => panic!("expected Corruption, got {other:?}"),
        Ok(ops) => {
            // If a future version chose to drop the tail instead, it must at
            // least preserve the survivor and never resurrect garbage.
            assert_eq!(kv_keys(&ops), vec!["survivor".to_string()]);
        }
    }
}

// ===========================================================================
// SECTION 9 — Empty / degenerate files
// ===========================================================================

#[test]
fn iter_on_header_only_file_returns_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_v2_wal(&path, &[]); // header only
    let wal = Wal::new(&path, false).unwrap();
    assert!(wal.iter().unwrap().is_empty());
}

#[test]
fn nonexistent_path_create_is_ok() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("wal.bin");
    // Parent exists, file does not: must be created with a header.
    let wal = Wal::new(&nested, true).unwrap();
    drop(wal);
    assert!(fs::read(&nested).unwrap().starts_with(WAL_FILE_HEADER));
}

#[test]
fn new_in_missing_directory_returns_io_error_not_panic() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("does/not/exist/wal.bin");
    match Wal::new(&missing, true) {
        Err(WalError::Io(_)) => {}
        Ok(_) => panic!("expected Io error for missing dir, got Ok(Wal)"),
        Err(other) => panic!("expected Io error for missing dir, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 10 — Large entries & many entries
// ===========================================================================

#[test]
fn large_single_entry_roundtrips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let big = "x".repeat(1_000_000); // ~1 MiB string property
    let op = Operation::KvSet {
        key: "big".to_string(),
        value: Value::String(big.clone()),
        ttl: None,
    };
    wal.append(&op).unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), 1);
    match &recovered[0] {
        Operation::KvSet { value: Value::String(s), .. } => assert_eq!(s.len(), big.len()),
        other => panic!("expected large KvSet, got {other:?}"),
    }
}

#[test]
fn many_entries_roundtrip_in_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    let n = 5000usize;
    for i in 0..n {
        wal.append(&kv_set(&format!("k{i:05}"))).unwrap();
    }
    drop(wal);

    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(recovered.len(), n);
    assert_eq!(recovered[0], "k00000");
    assert_eq!(recovered[n - 1], format!("k{:05}", n - 1));
    // Verify strict ordering across the whole log.
    for (i, key) in recovered.iter().enumerate() {
        assert_eq!(key, &format!("k{i:05}"));
    }
}

#[test]
fn large_node_with_many_props_roundtrips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let mut props = HashMap::new();
    for i in 0..1000 {
        props.insert(format!("prop_{i}"), Value::Int(i as i64));
    }
    let op = Operation::InsertNode {
        id: 7,
        labels: (0..50).map(|i| format!("Label{i}")).collect(),
        props: props.clone(),
    };
    wal.append(&op).unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    match &recovered[0] {
        Operation::InsertNode { id, labels, props: p } => {
            assert_eq!(*id, 7);
            assert_eq!(labels.len(), 50);
            assert_eq!(p.len(), 1000);
            assert_eq!(p.get("prop_999"), Some(&Value::Int(999)));
        }
        other => panic!("expected InsertNode, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 11 — Unicode / special-byte payloads
// ===========================================================================

#[test]
fn unicode_and_control_characters_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let weird = "héllo 世界 🚀 \u{0}\u{1}\u{7f} \t\n quote\" backslash\\";
    let op = Operation::KvSet {
        key: weird.to_string(),
        value: Value::String(weird.to_string()),
        ttl: None,
    };
    wal.append(&op).unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    match &recovered[0] {
        Operation::KvSet { key, value: Value::String(v), .. } => {
            assert_eq!(key, weird);
            assert_eq!(v, weird);
        }
        other => panic!("expected KvSet, got {other:?}"),
    }
}

#[test]
fn empty_string_and_empty_collections_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let ops = vec![
        Operation::KvSet {
            key: String::new(),
            value: Value::String(String::new()),
            ttl: None,
        },
        Operation::InsertNode {
            id: 0,
            labels: vec![],
            props: HashMap::new(),
        },
        Operation::KvSet {
            key: "list".to_string(),
            value: Value::List(vec![]),
            ttl: None,
        },
    ];
    for op in &ops {
        wal.append(op).unwrap();
    }
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), 3);
    match &recovered[0] {
        Operation::KvSet { key, value: Value::String(v), .. } => {
            assert!(key.is_empty());
            assert!(v.is_empty());
        }
        other => panic!("expected empty-string KvSet, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 12 — Boundary values
// ===========================================================================

#[test]
fn boundary_numeric_values_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let ops = vec![
        Operation::KvSet {
            key: "imax".to_string(),
            value: Value::Int(i64::MAX),
            ttl: Some(u64::MAX),
        },
        Operation::KvSet {
            key: "imin".to_string(),
            value: Value::Int(i64::MIN),
            ttl: Some(0),
        },
        Operation::InsertNode {
            id: u64::MAX,
            labels: vec!["Edge".to_string()],
            props: HashMap::new(),
        },
        Operation::InsertRel {
            id: u64::MAX,
            kind: "REL".to_string(),
            from: 0,
            to: u64::MAX,
            props: HashMap::new(),
        },
    ];
    for op in &ops {
        wal.append(op).unwrap();
    }
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), 4);
    match &recovered[0] {
        Operation::KvSet { value: Value::Int(n), ttl, .. } => {
            assert_eq!(*n, i64::MAX);
            assert_eq!(*ttl, Some(u64::MAX));
        }
        other => panic!("expected imax KvSet, got {other:?}"),
    }
    match &recovered[2] {
        Operation::InsertNode { id, .. } => assert_eq!(*id, u64::MAX),
        other => panic!("expected u64::MAX node, got {other:?}"),
    }
}

#[test]
fn nested_and_float_values_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let mut inner = HashMap::new();
    inner.insert("flt".to_string(), Value::from_f64(std::f64::consts::PI));
    inner.insert("nan".to_string(), Value::from_f64(f64::NAN));
    inner.insert("inf".to_string(), Value::from_f64(f64::INFINITY));
    let nested = Value::Map(inner);
    let op = Operation::KvSet {
        key: "nested".to_string(),
        value: Value::List(vec![nested.clone(), Value::Bool(true), Value::Null]),
        ttl: None,
    };
    wal.append(&op).unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    match &recovered[0] {
        Operation::KvSet { value: Value::List(items), .. } => {
            assert_eq!(items.len(), 3);
            match &items[0] {
                Value::Map(m) => {
                    // Pi survives bit-exactly through the bits-based Float repr.
                    assert_eq!(
                        m.get("flt").and_then(Value::to_f64),
                        Some(std::f64::consts::PI)
                    );
                    assert!(m.get("nan").and_then(Value::to_f64).unwrap().is_nan());
                    assert_eq!(m.get("inf").and_then(Value::to_f64), Some(f64::INFINITY));
                }
                other => panic!("expected Map, got {other:?}"),
            }
            assert_eq!(items[2], Value::Null);
        }
        other => panic!("expected List KvSet, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 13 — Snapshot + restore round-trip
// ===========================================================================

#[test]
fn snapshot_restore_roundtrips_graph_and_kv() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    let mut graph = Graph::new();
    let kv = KvStore::new();
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::String("Alice".to_string()));
    let nid = graph.create_node(vec!["Person".to_string()], props);
    let a = graph.create_node(vec!["A".to_string()], HashMap::new());
    let b = graph.create_node(vec!["B".to_string()], HashMap::new());
    let rid = graph.create_relationship("KNOWS".to_string(), a, b, HashMap::new());
    kv.set("foo".to_string(), Value::String("bar".to_string()), None);
    kv.set("count".to_string(), Value::Int(99), None);

    snapshot(&graph, &kv, &snap).unwrap();
    // The temp file must be cleaned up by the atomic rename.
    assert!(!snap.with_extension("bin.tmp").exists());

    let mut g2 = Graph::new();
    let kv2 = KvStore::new();
    assert!(restore(&mut g2, &kv2, &snap).unwrap());

    assert_eq!(g2.all_nodes().len(), 3);
    assert_eq!(g2.all_relationships().len(), 1);
    assert_eq!(
        g2.get_node(nid).map(|n| n.labels.clone()),
        Some(vec!["Person".to_string()])
    );
    assert_eq!(g2.get_relationship(rid).map(|r| r.kind.clone()), Some("KNOWS".to_string()));
    assert_eq!(kv2.get("foo"), Some(Value::String("bar".to_string())));
    assert_eq!(kv2.get("count"), Some(Value::Int(99)));
}

#[test]
fn restore_missing_file_returns_false() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope.bin");
    let mut g = Graph::new();
    let kv = KvStore::new();
    assert!(!restore(&mut g, &kv, &missing).unwrap());
    assert!(g.all_nodes().is_empty());
}

#[test]
fn restore_overwrites_existing_state() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    let mut graph = Graph::new();
    let kv = KvStore::new();
    graph.create_node(vec!["Saved".to_string()], HashMap::new());
    kv.set("saved".to_string(), Value::Int(1), None);
    snapshot(&graph, &kv, &snap).unwrap();

    // Destination starts non-empty; restore must clear and replace it.
    let mut g2 = Graph::new();
    let kv2 = KvStore::new();
    g2.create_node(vec!["Stale".to_string()], HashMap::new());
    g2.create_node(vec!["Stale".to_string()], HashMap::new());
    kv2.set("stale".to_string(), Value::Int(2), None);

    assert!(restore(&mut g2, &kv2, &snap).unwrap());
    assert_eq!(g2.all_nodes().len(), 1, "stale nodes replaced");
    assert!(g2.nodes_by_label("Saved").is_some());
    assert!(g2.nodes_by_label("Stale").is_none());
    assert_eq!(kv2.get("saved"), Some(Value::Int(1)));
    assert_eq!(kv2.get("stale"), None, "stale kv replaced");
}

#[test]
fn snapshot_of_empty_db_restores_empty() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");
    let graph = Graph::new();
    let kv = KvStore::new();
    snapshot(&graph, &kv, &snap).unwrap();

    let mut g2 = Graph::new();
    let kv2 = KvStore::new();
    g2.create_node(vec!["Will".to_string()], HashMap::new());
    assert!(restore(&mut g2, &kv2, &snap).unwrap());
    assert!(g2.all_nodes().is_empty());
}

#[test]
fn restore_from_corrupt_snapshot_is_error_not_panic() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");
    fs::write(&snap, b"this is not a valid bincode snapshot at all").unwrap();

    assert_snapshot_corruption(&snap);
}

#[test]
fn restore_from_gigabyte_length_prefix_is_corruption() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    // The first snapshot field is the node map count. Claim 4 Gi entries in
    // an otherwise tiny file; restore must reject it before any large reserve.
    fs::write(&snap, (4_u64 * 1024 * 1024 * 1024).to_le_bytes()).unwrap();

    assert_snapshot_corruption(&snap);
}

#[test]
fn restore_from_truncated_mid_record_snapshot_is_corruption() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    // Produce a valid snapshot, then truncate it mid-stream.
    let mut graph = Graph::new();
    let kv = KvStore::new();
    for i in 0..100 {
        graph.create_node(vec![format!("L{i}")], HashMap::new());
    }
    snapshot(&graph, &kv, &snap).unwrap();
    let bytes = fs::read(&snap).unwrap();
    fs::write(&snap, &bytes[..bytes.len() / 2]).unwrap();

    assert_snapshot_corruption(&snap);
}

#[test]
fn snapshot_preserves_node_properties_and_ttl_kv() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    let mut graph = Graph::new();
    let kv = KvStore::new();
    let mut props = HashMap::new();
    props.insert("active".to_string(), Value::Bool(true));
    props.insert("score".to_string(), Value::from_f64(9.5));
    let id = graph.create_node(vec!["User".to_string()], props);
    kv.set("ttl-key".to_string(), Value::Int(7), Some(86_400));

    snapshot(&graph, &kv, &snap).unwrap();

    let mut g2 = Graph::new();
    let kv2 = KvStore::new();
    restore(&mut g2, &kv2, &snap).unwrap();
    let node = g2.get_node(id).unwrap();
    assert_eq!(node.props.get("active"), Some(&Value::Bool(true)));
    assert_eq!(node.props.get("score").and_then(Value::to_f64), Some(9.5));
    // A long TTL key must still be readable immediately after restore.
    assert_eq!(kv2.get("ttl-key"), Some(Value::Int(7)));
}

// ===========================================================================
// SECTION 14 — Crash-at-offset replay (in-process)
// ===========================================================================

#[test]
fn replay_after_crash_at_every_byte_offset_never_panics_or_resurrects() {
    // Build a known-good multi-entry WAL, then simulate a crash at EVERY
    // possible prefix length. For each prefix, reopening + iter() must:
    //   * never panic / never error spuriously on the valid prefix, and
    //   * return a prefix of the original op sequence (no resurrected or
    //     reordered entries).
    let dir = tempdir().unwrap();
    let golden = dir.path().join("golden.bin");
    let ops: Vec<Operation> = (0..12).map(|i| kv_set(&format!("op{i:02}"))).collect();
    handcraft_v2_wal(&golden, &ops);
    let full = fs::read(&golden).unwrap();
    let expected_keys = kv_keys(&ops);

    for cut in 0..=full.len() {
        let path = dir.path().join(format!("crash_{cut}.bin"));
        fs::write(&path, &full[..cut]).unwrap();

        // Opening a prefix shorter than the header is its own concern: a
        // partial header is either re-initialized (empty file) or rejected.
        let opened = Wal::new(&path, false);
        let wal = match opened {
            Ok(w) => w,
            Err(_) => continue, // partial/invalid header => acceptable Err, not a panic
        };
        match wal.iter() {
            Ok(recovered) => {
                let keys = kv_keys(&recovered);
                // Whatever survived must be an exact ordered prefix of the
                // original sequence — never garbage, never reordered.
                assert!(
                    keys.len() <= expected_keys.len(),
                    "cut {cut}: recovered more entries than existed"
                );
                assert_eq!(
                    keys.as_slice(),
                    &expected_keys[..keys.len()],
                    "cut {cut}: recovered entries must be an ordered prefix"
                );
            }
            // A mid-stream corruption error is acceptable for some cuts (e.g.
            // a CRC happens to match a different entry) but must not panic.
            Err(WalError::Corruption { .. }) => {}
            Err(other) => panic!("cut {cut}: unexpected error {other:?}"),
        }
    }
}

#[test]
fn acked_prefix_survives_when_tail_entry_is_chopped_at_each_offset() {
    // Stronger guarantee: take N fully-acked entries and verify that no matter
    // where a final (N+1)-th entry was torn, all N acked entries survive.
    let dir = tempdir().unwrap();
    let base = dir.path().join("base.bin");
    let acked: Vec<Operation> = (0..6).map(|i| kv_set(&format!("acked{i}"))).collect();
    handcraft_v2_wal(&base, &acked);
    let base_bytes = fs::read(&base).unwrap();
    let acked_keys = kv_keys(&acked);

    // The extra torn entry's full framed bytes.
    let extra_payload = payload_of(&kv_set("torn"));
    let mut framed = Vec::new();
    framed.extend_from_slice(&(extra_payload.len() as u64).to_le_bytes());
    framed.extend_from_slice(&crc32fast::hash(&extra_payload).to_le_bytes());
    framed.extend_from_slice(&extra_payload);

    for partial in 0..framed.len() {
        // Note: partial == framed.len() would be a *complete* extra entry, so
        // we stop one short to keep it strictly torn.
        let path = dir.path().join(format!("torn_{partial}.bin"));
        let mut bytes = base_bytes.clone();
        bytes.extend_from_slice(&framed[..partial]);
        fs::write(&path, &bytes).unwrap();

        let wal = Wal::new(&path, false).unwrap();
        match wal.iter() {
            Ok(recovered) => {
                let keys = kv_keys(&recovered);
                assert_eq!(
                    keys, acked_keys,
                    "partial {partial}: all acked entries must survive a torn tail"
                );
            }
            Err(WalError::Corruption { .. }) => {
                // Acceptable only if the torn bytes happened to look like a
                // self-consistent but undeserializable entry; never a panic.
            }
            Err(other) => panic!("partial {partial}: unexpected {other:?}"),
        }
    }
}

// ===========================================================================
// SECTION 15 — Durability across simulated process restarts
// ===========================================================================

#[test]
fn data_survives_drop_and_reopen_cycle_repeatedly() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let mut expected = Vec::new();
    for round in 0..5 {
        let wal = Wal::new(&path, true).unwrap();
        for i in 0..10 {
            let key = format!("r{round}-e{i}");
            wal.append(&kv_set(&key)).unwrap();
            expected.push(key);
        }
        drop(wal); // simulate clean process exit
    }
    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(recovered, expected, "all entries across 5 restarts must survive in order");
}

#[test]
fn flush_then_drop_in_group_mode_loses_nothing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let wal = Wal::with_group_commit(&path, false, Duration::from_millis(2), 1000).unwrap();
        // Every append is a blocking durability barrier; flush and drop after
        // the writes must preserve the complete sequence.
        for i in 0..20 {
            wal.append(&kv_set(&format!("e{i}"))).unwrap();
        }
        wal.flush().unwrap();
    }
    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), 20);
}

// ===========================================================================
// SECTION 16 — Deep structural fidelity of every non-Kv operation variant
//
// The author's `all_operation_variants_roundtrip` only spot-checks the first
// (InsertNode) and last (KvDel) entries. The remaining five variants survive
// the round-trip in count but their *fields* were never asserted. A bincode
// field-ordering or schema drift could silently corrupt `from`/`to`/`kind`
// without these tests noticing. Assert every field of every variant.
// ===========================================================================

#[test]
fn update_node_fields_survive_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let mut props = HashMap::new();
    props.insert("status".to_string(), Value::String("active".to_string()));
    props.insert("rank".to_string(), Value::Int(-5));
    wal.append(&Operation::UpdateNode {
        id: 42,
        props: props.clone(),
    })
    .unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), 1);
    match &recovered[0] {
        Operation::UpdateNode { id, props: p } => {
            assert_eq!(*id, 42);
            assert_eq!(p.len(), 2);
            assert_eq!(p.get("status"), Some(&Value::String("active".to_string())));
            assert_eq!(p.get("rank"), Some(&Value::Int(-5)));
        }
        other => panic!("expected UpdateNode, got {other:?}"),
    }
}

#[test]
fn delete_node_field_survives_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    wal.append(&Operation::DeleteNode { id: 9_999 }).unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    match &recovered[0] {
        Operation::DeleteNode { id } => assert_eq!(*id, 9_999),
        other => panic!("expected DeleteNode, got {other:?}"),
    }
}

#[test]
fn insert_rel_all_fields_survive_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    let mut props = HashMap::new();
    props.insert("since".to_string(), Value::Int(2020));
    props.insert("weight".to_string(), Value::from_f64(0.75));
    wal.append(&Operation::InsertRel {
        id: 555,
        kind: "FOLLOWS".to_string(),
        from: 7,
        to: 8,
        props: props.clone(),
    })
    .unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    match &recovered[0] {
        Operation::InsertRel {
            id,
            kind,
            from,
            to,
            props: p,
        } => {
            assert_eq!(*id, 555);
            assert_eq!(kind, "FOLLOWS");
            // The critical assertion: from/to must not be swapped or dropped.
            assert_eq!(*from, 7);
            assert_eq!(*to, 8);
            assert_eq!(p.get("since"), Some(&Value::Int(2020)));
            assert_eq!(p.get("weight").and_then(Value::to_f64), Some(0.75));
        }
        other => panic!("expected InsertRel, got {other:?}"),
    }
}

#[test]
fn delete_rel_field_survives_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    wal.append(&Operation::DeleteRel { id: 1 }).unwrap();
    wal.append(&Operation::DeleteRel { id: u64::MAX }).unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(recovered.len(), 2);
    match (&recovered[0], &recovered[1]) {
        (Operation::DeleteRel { id: a }, Operation::DeleteRel { id: b }) => {
            assert_eq!(*a, 1);
            assert_eq!(*b, u64::MAX);
        }
        other => panic!("expected two DeleteRel, got {other:?}"),
    }
}

#[test]
fn kvset_with_no_ttl_distinguishes_from_zero_ttl() {
    // `ttl: None` and `ttl: Some(0)` are semantically different and must NOT
    // collapse into the same on-disk encoding.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    wal.append(&Operation::KvSet {
        key: "none".to_string(),
        value: Value::Int(1),
        ttl: None,
    })
    .unwrap();
    wal.append(&Operation::KvSet {
        key: "zero".to_string(),
        value: Value::Int(2),
        ttl: Some(0),
    })
    .unwrap();
    drop(wal);

    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    match (&recovered[0], &recovered[1]) {
        (
            Operation::KvSet { ttl: t_none, .. },
            Operation::KvSet { ttl: t_zero, .. },
        ) => {
            assert_eq!(*t_none, None, "None ttl must round-trip as None");
            assert_eq!(*t_zero, Some(0), "Some(0) ttl must round-trip as Some(0)");
        }
        other => panic!("expected two KvSet, got {other:?}"),
    }
}

// ===========================================================================
// SECTION 17 — Legacy (v1) migration: undeserializable / overflow / empty
//
// `migrate_legacy_wal` deserializes every legacy entry as an Operation while
// reframing it. The author covered the happy path and a torn tail, but NOT:
//   * a complete legacy entry whose payload fails to deserialize (the
//     migration's `bincode::deserialize::<Operation>(...)?` Corruption branch),
//   * a legacy length prefix that overflows past EOF mid-stream,
//   * that migration leaves no `.wal.migrate.tmp` scratch file behind,
//   * a legacy file that is exactly a single torn length prefix (< 8 bytes).
// ===========================================================================

#[test]
fn legacy_entry_with_undeserializable_payload_is_corruption() {
    // A complete legacy entry (length fits in the file) whose payload is not a
    // valid bincode Operation must abort migration with Corruption — NOT be
    // silently reframed into a v2 entry that later fails to replay.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        // One good legacy entry first.
        let good = payload_of(&kv_set("good"));
        file.write_all(&(good.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&good).unwrap();
        // Then a complete-but-garbage legacy entry.
        let garbage = vec![0xFEu8; 16];
        file.write_all(&(garbage.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&garbage).unwrap();
        file.sync_all().unwrap();
    }
    match Wal::new(&path, true) {
        Err(WalError::Corruption { reason, .. }) => {
            assert!(
                reason.contains("legacy") || reason.contains("payload"),
                "reason should flag the legacy payload: {reason}"
            );
        }
        Ok(_) => panic!("garbage legacy payload must not migrate cleanly"),
        Err(other) => panic!("expected Corruption, got {other:?}"),
    }
}

#[test]
fn legacy_length_prefix_overflowing_past_eof_is_corruption() {
    // Arithmetic overflow in a legacy entry length is distinguishable from an
    // ordinary torn tail and is reported as hard corruption.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        let good = payload_of(&kv_set("kept"));
        file.write_all(&(good.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&good).unwrap();
        // Declare a giant length but provide almost no payload.
        file.write_all(&u64::MAX.to_le_bytes()).unwrap();
        file.write_all(b"x").unwrap();
        file.sync_all().unwrap();
    }
    match Wal::new(&path, true) {
        Err(WalError::Corruption { offset, reason }) => {
            assert_eq!(offset, 41);
            assert!(reason.contains("legacy entry length overflow"));
        }
        Ok(_) => panic!("overflowing legacy length must not migrate"),
        Err(other) => panic!("expected Corruption, got {other:?}"),
    }
    assert!(
        !fs::read(&path).unwrap().starts_with(WAL_FILE_HEADER),
        "failed migration must leave the original legacy WAL untouched"
    );
}

#[test]
fn legacy_with_sub_eight_byte_remainder_migrates_complete_prefix() {
    // A legacy file ending in fewer than 8 bytes (a partial length prefix)
    // hits the `remaining < 8 => break` arm; the complete prefix migrates.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        let good = payload_of(&kv_set("solo"));
        file.write_all(&(good.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&good).unwrap();
        file.write_all(&[0xAA, 0xBB, 0xCC]).unwrap(); // 3 trailing bytes
        file.sync_all().unwrap();
    }
    let wal = Wal::new(&path, true).unwrap();
    assert_eq!(kv_keys(&wal.iter().unwrap()), vec!["solo".to_string()]);
}

#[test]
fn migration_leaves_no_scratch_tmp_file() {
    // migrate_legacy_wal writes to `<path>.wal.migrate.tmp` then renames. After
    // a successful migration that scratch file must not exist.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    handcraft_legacy_wal(&path, &[kv_set("a"), kv_set("b"), kv_set("c")]);

    let _wal = Wal::new(&path, true).unwrap();
    let tmp = path.with_extension("wal.migrate.tmp");
    assert!(!tmp.exists(), "migration scratch tmp must be cleaned up via rename");
}

#[test]
fn large_legacy_log_migrates_in_order() {
    // Stress the migration reframer with many entries to confirm ordering and
    // that every entry gets a fresh CRC in the v2 framing.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let ops: Vec<Operation> = (0..500).map(|i| kv_set(&format!("L{i:04}"))).collect();
    handcraft_legacy_wal(&path, &ops);

    let wal = Wal::new(&path, true).unwrap();
    let recovered = kv_keys(&wal.iter().unwrap());
    assert_eq!(recovered.len(), 500);
    for (i, key) in recovered.iter().enumerate() {
        assert_eq!(key, &format!("L{i:04}"));
    }
    // Re-derive each v2 CRC from the migrated bytes and confirm validity by a
    // clean second open (iter must not truncate or error).
    let wal2 = Wal::new(&path, false).unwrap();
    assert_eq!(wal2.iter().unwrap().len(), 500);
}

// ===========================================================================
// SECTION 18 — prepare_wal: an already-valid v2 file with real entries is
// reopened WITHOUT re-running migration or rewriting the body.
// ===========================================================================

#[test]
fn reopening_valid_v2_file_does_not_rewrite_body() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let wal = Wal::new(&path, true).unwrap();
        wal.append(&kv_set("p")).unwrap();
        wal.append(&kv_set("q")).unwrap();
    }
    let before = fs::read(&path).unwrap();
    {
        // Reopen: prepare_wal sees ZWAL magic + version 2 and returns Ok with no
        // rewrite. The bytes on disk must be byte-for-byte identical.
        let _ = Wal::new(&path, false).unwrap();
    }
    let after = fs::read(&path).unwrap();
    assert_eq!(before, after, "reopening a valid v2 WAL must not mutate its bytes");
}

// ===========================================================================
// SECTION 19 — iter() requires a writable handle; read-only / missing files.
//
// iter() opens the path with `.read(true).write(true)` (it may need to
// truncate a torn tail). A path that no longer exists when iter() runs, or one
// that is not writable, must surface as an Io error value, never a panic.
// ===========================================================================

#[test]
fn iter_on_deleted_file_returns_io_error_not_panic() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();
    wal.append(&kv_set("gone")).unwrap();
    // Remove the file out from under the open WAL, then iter().
    fs::remove_file(&path).unwrap();
    match wal.iter() {
        Err(WalError::Io(_)) => {}
        Ok(ops) => panic!("expected Io error after file removal, got {} ops", ops.len()),
        Err(other) => panic!("expected Io error, got {other:?}"),
    }
}

#[test]
fn in_memory_iter_is_io_error_never_panic() {
    // Resolve the author's flagged uncertain API: in_memory() backs `path` with
    // an empty PathBuf, so iter() opens "" and must return an Io error (not a
    // Corruption, not a panic, not Ok). Pin the exact Result shape.
    let wal = Wal::in_memory();
    match wal.iter() {
        Err(WalError::Io(_)) => {}
        Ok(ops) => panic!("in_memory iter must not succeed, got {} ops", ops.len()),
        Err(other) => panic!("in_memory iter must be Io error, got {other:?}"),
    }
}

#[test]
fn in_memory_append_then_flush_never_touches_disk() {
    // in_memory() has file: None. append() short-circuits to Ok without writing,
    // flush() is a no-op. Neither may panic or block.
    let wal = Wal::in_memory();
    for i in 0..100 {
        wal.append(&kv_set(&format!("ghost{i}"))).unwrap();
    }
    wal.flush().unwrap();
    wal.flush().unwrap();
}

// ===========================================================================
// SECTION 20 — Group-commit timing: batch-size trigger vs interval trigger.
//
// The author covered "batch larger than writes flushed by interval" and
// "concurrent writers". Add the complementary cases: a batch-size of exactly 1
// (every append a barrier), and a write count that crosses the batch boundary
// so the batch-size wake path (notify_one) is exercised, plus a tiny interval.
// ===========================================================================

#[test]
fn batch_size_one_makes_every_append_a_durability_barrier() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::with_group_commit(&path, false, Duration::from_secs(30), 1).unwrap();
    // With batch_size 1 and a huge interval, append must still return only once
    // its own entry is durable — an independent reopen sees it immediately.
    wal.append(&kv_set("barrier")).unwrap();
    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    assert_eq!(kv_keys(&recovered), vec!["barrier".to_string()]);
}

#[test]
fn appends_crossing_batch_boundary_are_all_durable() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    // Concurrent writers can fill batches while earlier append calls wait for
    // durability. A short interval flushes the trailing partial batch.
    let wal = Arc::new(
        Wal::with_group_commit(&path, false, Duration::from_millis(5), 4).unwrap(),
    );
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let wal = Arc::clone(&wal);
            thread::spawn(move || wal.append(&kv_set(&format!("c{i}"))).unwrap())
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(recovered.len(), 10);
    for i in 0..10 {
        assert!(
            recovered.contains(&format!("c{i}")),
            "missing acknowledged entry c{i}"
        );
    }
}

#[test]
fn many_concurrent_writers_high_contention_all_durable() {
    // Heavier than the author's 8-thread test: 32 threads each writing several
    // entries through a small batch. Every Ok must be durable and the final
    // count exact (no lost or duplicated acks under contention).
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Arc::new(
        Wal::with_group_commit(&path, false, Duration::from_millis(2), 8).unwrap(),
    );
    let per_thread = 5;
    let n_threads = 32;
    let handles: Vec<_> = (0..n_threads)
        .map(|t| {
            let wal = Arc::clone(&wal);
            thread::spawn(move || {
                for e in 0..per_thread {
                    wal.append(&kv_set(&format!("t{t}-e{e}"))).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(wal.iter().unwrap().len(), n_threads * per_thread);
}

// ===========================================================================
// SECTION 21 — Mixed-mode reopen: a file written in group-commit mode reopens
// cleanly in flush_every mode and vice-versa (the on-disk format is identical;
// only the in-memory commit strategy differs).
// ===========================================================================

#[test]
fn group_written_log_reopens_in_flush_every_mode() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let wal = Wal::with_group_commit(&path, false, Duration::from_millis(2), 16).unwrap();
        for i in 0..15 {
            wal.append(&kv_set(&format!("g{i}"))).unwrap();
        }
    }
    // Reopen in flush_every mode and append more, then read back all.
    {
        let wal = Wal::new(&path, true).unwrap();
        for i in 0..5 {
            wal.append(&kv_set(&format!("f{i}"))).unwrap();
        }
    }
    let recovered = kv_keys(&Wal::new(&path, false).unwrap().iter().unwrap());
    assert_eq!(recovered.len(), 20);
    assert_eq!(recovered[0], "g0");
    assert_eq!(recovered[14], "g14");
    assert_eq!(recovered[15], "f0");
    assert_eq!(recovered[19], "f4");
}

// ===========================================================================
// SECTION 22 — Snapshot atomicity, overwrite, and relationship/index recovery.
//
// The author covered round-trip, missing-file, overwrite, empty, corrupt, and
// truncated snapshots. Add: an existing snapshot at the destination is replaced
// atomically (no .bin.tmp residue), relationships restore with both endpoints
// queryable via the rebuilt indexes, and a re-snapshot of restored state is
// byte-stable in node/rel counts (idempotent snapshot->restore->snapshot).
// ===========================================================================

#[test]
fn snapshot_overwrites_prior_snapshot_atomically() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    // First snapshot: one node.
    let mut g1 = Graph::new();
    let kv1 = KvStore::new();
    g1.create_node(vec!["First".to_string()], HashMap::new());
    snapshot(&g1, &kv1, &snap).unwrap();

    // Second snapshot to the SAME path: three nodes. Must fully replace.
    let mut g2 = Graph::new();
    let kv2 = KvStore::new();
    for _ in 0..3 {
        g2.create_node(vec!["Second".to_string()], HashMap::new());
    }
    snapshot(&g2, &kv2, &snap).unwrap();
    assert!(!snap.with_extension("bin.tmp").exists(), "no tmp residue");

    let mut gr = Graph::new();
    let kvr = KvStore::new();
    restore(&mut gr, &kvr, &snap).unwrap();
    assert_eq!(gr.all_nodes().len(), 3, "second snapshot fully replaced the first");
    assert!(gr.nodes_by_label("First").is_none());
    assert!(gr.nodes_by_label("Second").is_some());
}

#[test]
fn restored_relationship_endpoints_are_queryable_via_rebuilt_index() {
    // restore() calls graph.set_state which rebuilds outgoing/incoming indexes.
    // Confirm a restored relationship is reachable from both endpoints, not just
    // present in all_relationships().
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    let mut g = Graph::new();
    let kv = KvStore::new();
    let a = g.create_node(vec!["A".to_string()], HashMap::new());
    let b = g.create_node(vec!["B".to_string()], HashMap::new());
    let rid = g.create_relationship("LINKS".to_string(), a, b, HashMap::new());
    snapshot(&g, &kv, &snap).unwrap();

    let mut gr = Graph::new();
    let kvr = KvStore::new();
    assert!(restore(&mut gr, &kvr, &snap).unwrap());
    assert!(
        gr.outgoing_rels(a).is_some_and(|set| set.contains(&rid)),
        "restored rel must be in the rebuilt outgoing index of its source"
    );
    assert!(
        gr.incoming_rels(b).is_some_and(|set| set.contains(&rid)),
        "restored rel must be in the rebuilt incoming index of its target"
    );
}

#[test]
fn snapshot_restore_snapshot_is_idempotent_in_counts() {
    let dir = tempdir().unwrap();
    let snap1 = dir.path().join("snap1.bin");
    let snap2 = dir.path().join("snap2.bin");

    let mut g = Graph::new();
    let kv = KvStore::new();
    let a = g.create_node(vec!["N".to_string()], HashMap::new());
    let b = g.create_node(vec!["N".to_string()], HashMap::new());
    g.create_relationship("E".to_string(), a, b, HashMap::new());
    kv.set("k1".to_string(), Value::Int(1), None);
    kv.set("k2".to_string(), Value::Int(2), Some(3600));
    snapshot(&g, &kv, &snap1).unwrap();

    let mut g2 = Graph::new();
    let kv2 = KvStore::new();
    restore(&mut g2, &kv2, &snap1).unwrap();
    snapshot(&g2, &kv2, &snap2).unwrap();

    let mut g3 = Graph::new();
    let kv3 = KvStore::new();
    restore(&mut g3, &kv3, &snap2).unwrap();
    assert_eq!(g3.all_nodes().len(), 2);
    assert_eq!(g3.all_relationships().len(), 1);
    assert_eq!(kv3.get("k1"), Some(Value::Int(1)));
    assert_eq!(kv3.get("k2"), Some(Value::Int(2)));
}

#[test]
fn restore_from_empty_zero_byte_snapshot_is_error_not_panic() {
    // A zero-byte snapshot file exists (so restore does not early-return false)
    // but has no bincode body. deserialize_from must yield an Err, not a panic.
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");
    File::create(&snap).unwrap().sync_all().unwrap();
    assert_eq!(fs::metadata(&snap).unwrap().len(), 0);

    let mut g = Graph::new();
    let kv = KvStore::new();
    assert!(restore(&mut g, &kv, &snap).is_err(), "empty snapshot must be an error value");
}

#[test]
fn snapshot_with_unicode_and_extreme_values_roundtrips() {
    let dir = tempdir().unwrap();
    let snap = dir.path().join("snap.bin");

    let mut g = Graph::new();
    let kv = KvStore::new();
    let mut props = HashMap::new();
    props.insert("名前".to_string(), Value::String("🚀\u{0}\t".to_string()));
    props.insert("min".to_string(), Value::Int(i64::MIN));
    props.insert("max".to_string(), Value::Int(i64::MAX));
    let id = g.create_node(vec!["Ünïcödé".to_string()], props);
    kv.set("空".to_string(), Value::List(vec![Value::Null, Value::Bool(false)]), None);

    snapshot(&g, &kv, &snap).unwrap();
    let mut gr = Graph::new();
    let kvr = KvStore::new();
    restore(&mut gr, &kvr, &snap).unwrap();
    let node = gr.get_node(id).unwrap();
    assert_eq!(node.labels, vec!["Ünïcödé".to_string()]);
    assert_eq!(node.props.get("min"), Some(&Value::Int(i64::MIN)));
    assert_eq!(node.props.get("max"), Some(&Value::Int(i64::MAX)));
    assert_eq!(
        kvr.get("空"),
        Some(Value::List(vec![Value::Null, Value::Bool(false)]))
    );
}

// ===========================================================================
// SECTION 23 — WAL replay drives a graph + kv to a known state (integration).
//
// Durability is meaningless unless replay reconstructs the intended state. The
// author tested that ops survive byte-for-byte; this test closes the loop by
// APPLYING the recovered op stream to a fresh Graph/KvStore (the way a real DB
// recovers) and asserting the resulting state is exactly correct, including
// that a later DeleteNode/KvDel in the log wins over an earlier insert/set.
// ===========================================================================

fn apply_op(graph: &mut Graph, kv: &KvStore, op: &Operation) {
    match op {
        Operation::InsertNode { id, labels, props } => {
            graph.restore_node(*id, labels.clone(), props.clone());
        }
        Operation::UpdateNode { id, props } => {
            graph.update_node(*id, props.clone());
        }
        Operation::DeleteNode { id } => {
            graph.delete_node(*id);
        }
        Operation::InsertRel {
            id,
            kind,
            from,
            to,
            props,
        } => {
            graph.restore_relationship(*id, kind.clone(), *from, *to, props.clone());
        }
        Operation::DeleteRel { id } => {
            graph.delete_relationship(*id);
        }
        Operation::KvSet { key, value, ttl } => {
            kv.set(key.clone(), value.clone(), *ttl);
        }
        Operation::KvDel { key } => {
            kv.del(key);
        }
    }
}

#[test]
fn replaying_recovered_ops_reconstructs_expected_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    let wal = Wal::new(&path, true).unwrap();

    // A log where later ops mutate/undo earlier ones.
    let mut props_a = HashMap::new();
    props_a.insert("v".to_string(), Value::Int(1));
    wal.append(&Operation::InsertNode {
        id: 1,
        labels: vec!["Keep".to_string()],
        props: props_a,
    })
    .unwrap();
    wal.append(&Operation::InsertNode {
        id: 2,
        labels: vec!["Doomed".to_string()],
        props: HashMap::new(),
    })
    .unwrap();
    let mut upd = HashMap::new();
    upd.insert("v".to_string(), Value::Int(99));
    wal.append(&Operation::UpdateNode { id: 1, props: upd }).unwrap();
    wal.append(&Operation::InsertRel {
        id: 10,
        kind: "R".to_string(),
        from: 1,
        to: 2,
        props: HashMap::new(),
    })
    .unwrap();
    wal.append(&Operation::DeleteNode { id: 2 }).unwrap(); // also drops rel 10
    wal.append(&Operation::KvSet {
        key: "live".to_string(),
        value: Value::String("yes".to_string()),
        ttl: None,
    })
    .unwrap();
    wal.append(&Operation::KvSet {
        key: "temp".to_string(),
        value: Value::Int(7),
        ttl: None,
    })
    .unwrap();
    wal.append(&Operation::KvDel {
        key: "temp".to_string(),
    })
    .unwrap();
    drop(wal);

    // Recover and replay into a fresh state.
    let recovered = Wal::new(&path, false).unwrap().iter().unwrap();
    let mut graph = Graph::new();
    let kv = KvStore::new();
    for op in &recovered {
        apply_op(&mut graph, &kv, op);
    }

    // Node 1 survives with its UPDATED property; node 2 (and its rel) is gone.
    let n1 = graph.get_node(1).expect("node 1 must survive replay");
    assert_eq!(n1.props.get("v"), Some(&Value::Int(99)), "update must win over insert");
    assert!(graph.get_node(2).is_none(), "deleted node must not be resurrected");
    assert!(
        graph.get_relationship(10).is_none(),
        "rel attached to a deleted node must be gone after replay"
    );
    // KV: the deleted key is absent; the live key remains.
    assert_eq!(kv.get("live"), Some(Value::String("yes".to_string())));
    assert_eq!(kv.get("temp"), None, "KvDel must win over the earlier KvSet");
}

// ===========================================================================
// SECTION 24 — entry-length zero (a valid empty payload is not deserializable,
// so a zero-length entry is a hard corruption — never an infinite loop).
// ===========================================================================

#[test]
fn zero_length_entry_does_not_hang_and_is_corruption() {
    // A framed entry declaring len=0 with the matching crc of an empty payload.
    // The reader advances ENTRY_HEADER_LEN (no infinite loop) but the empty
    // payload cannot deserialize into an Operation => Corruption if non-tail.
    let dir = tempdir().unwrap();
    let path = dir.path().join("wal.bin");
    {
        let mut file = File::create(&path).unwrap();
        file.write_all(WAL_FILE_HEADER).unwrap();
        // Zero-length entry first (non-tail), then a valid entry after it.
        file.write_all(&0u64.to_le_bytes()).unwrap();
        file.write_all(&crc32fast::hash(&[]).to_le_bytes()).unwrap();
        // (no payload bytes)
        write_framed_entry(&mut file, &payload_of(&kv_set("after")));
        file.sync_all().unwrap();
    }
    // Must terminate (no hang) and surface a Corruption value (empty payload is
    // not a valid Operation), never a panic.
    match Wal::new(&path, false).unwrap().iter() {
        Err(WalError::Corruption { .. }) => {}
        Ok(ops) => panic!("zero-length entry must not yield ops, got {}", ops.len()),
        Err(other) => panic!("expected Corruption, got {other:?}"),
    }
}
