//! `.graph` import and export through the public API: a damaged file is
//! refused with a clear error and leaves the database exactly as it was, in
//! memory and on disk; a good one replaces the graph and survives a reopen.

use std::path::Path;

use serde_json::Value as Json;
use zega::graph_file::{self, Error};
use zega::{Zega, ZegaError};

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden-v1.graph");
const PEOPLE: &str = "type Person { name: String age: Int }";

fn golden() -> Vec<u8> {
    std::fs::read(GOLDEN).unwrap()
}

fn export(zega: &Zega) -> Vec<u8> {
    let mut bytes = Vec::new();
    zega.export(&mut bytes).unwrap();
    bytes
}

/// A database that already holds something, so "unchanged" means something.
fn seeded(zega: &Zega) {
    zega.run_lang(PEOPLE, r#"mutation { Person(name: "Ada" && age: 36) { name } }"#)
        .unwrap();
    zega.run_lang(PEOPLE, r#"mutation { Person(name: "Grace" && age: 85) { name } }"#)
        .unwrap();
}

fn open(dir: &Path) -> Zega {
    Zega::open(dir.to_str().unwrap()).build().unwrap()
}

fn graph_error(result: Result<graph_file::ImportSummary, ZegaError>) -> Error {
    match result {
        Err(ZegaError::GraphFile(error)) => error,
        Err(other) => panic!("expected a .graph error, got {other}"),
        Ok(summary) => panic!("expected an error, the import succeeded: {summary:?}"),
    }
}

/// `(name, start)` of every section, then the end of the file.
fn section_starts(bytes: &[u8]) -> Vec<(String, usize)> {
    let mut starts = Vec::new();
    let mut at = 12;
    while at < bytes.len() {
        let tag = String::from_utf8_lossy(&bytes[at..at + 4]).to_string();
        let len = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
        starts.push((tag, at));
        at += 12 + len + 4;
    }
    assert_eq!(at, bytes.len());
    starts.push(("end of file".into(), at));
    starts
}

/// The offsets worth cutting at: inside the header, at every section
/// boundary, inside every section header, payload and checksum.
fn cuts(bytes: &[u8]) -> Vec<usize> {
    let starts = section_starts(bytes);
    let mut cuts = vec![0, 4, 8, 10];
    for window in starts.windows(2) {
        let (start, next) = (window[0].1, window[1].1);
        cuts.extend([start, start + 2, start + 12, (start + 12 + next - 4) / 2, next - 2]);
    }
    cuts.retain(|cut| *cut < bytes.len());
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

#[test]
fn golden_file_imports_into_a_database_with_every_value_type() {
    let zega = Zega::in_memory().build().unwrap();
    seeded(&zega);
    let summary = zega.import(&golden()[..]).unwrap();
    assert_eq!((summary.nodes, summary.relationships), (3, 2));
    let graph = zega.graph_json().unwrap();
    let names: Vec<&str> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect();
    assert_eq!(names, ["Calgary"], "the import replaced Ada and Grace");
    let schema = "type City { name: String population: Int }";
    let read = zega.run_lang(schema, "{ City { name population } }").unwrap();
    assert_eq!(read, serde_json::json!([{ "name": "Calgary", "population": 1306784 }]));
    // Exported again, it is the same graph: the content digest ignores only
    // the writer's name and the manifest metadata.
    let mut again = Vec::new();
    let export = zega.export(&mut again).unwrap();
    let reimported = Zega::in_memory().build().unwrap().import(&again[..]).unwrap();
    assert_eq!(export.content_sha256, reimported.content_sha256);
}

#[test]
fn truncation_anywhere_is_refused_and_changes_nothing() {
    let bytes = golden();
    let zega = Zega::in_memory().build().unwrap();
    seeded(&zega);
    let before = export(&zega);
    let json_before = zega.graph_json().unwrap();
    let cuts = cuts(&bytes);
    assert!(cuts.len() > 25, "{cuts:?}");
    for cut in cuts {
        let error = graph_error(zega.import(&bytes[..cut]));
        assert!(
            matches!(error, Error::Truncated { offset, .. } if offset == cut as u64),
            "cut at {cut}: {error}"
        );
        assert!(error.to_string().starts_with("truncated .graph file"), "{error}");
        assert_eq!(export(&zega), before, "cut at {cut} changed the graph");
    }
    assert_eq!(zega.graph_json().unwrap(), json_before);
}

#[test]
fn a_cut_at_each_section_boundary_names_the_missing_section() {
    let bytes = golden();
    let zega = Zega::in_memory().build().unwrap();
    let starts = section_starts(&bytes);
    let expected = ["manifest", "names", "schema", "nodes", "relationships", "done"];
    for ((_, start), name) in starts.iter().zip(expected) {
        let error = graph_error(zega.import(&bytes[..*start]));
        assert_eq!(
            error.to_string(),
            format!("truncated .graph file: it ends at byte {start}, where the {name} section should start")
        );
    }
    assert!(zega.is_empty().unwrap());
}

#[test]
fn a_bad_checksum_is_refused_and_changes_nothing() {
    let zega = Zega::in_memory().build().unwrap();
    seeded(&zega);
    let before = export(&zega);
    let golden = golden();
    for (tag, start) in section_starts(&golden) {
        if tag == "end of file" {
            continue;
        }
        let len = u64::from_le_bytes(golden[start + 4..start + 12].try_into().unwrap()) as usize;
        // One flipped bit in the payload's last byte, then in the stored checksum.
        for at in [start + 12 + len - 1, start + 12 + len] {
            let mut bytes = golden.clone();
            bytes[at] ^= 0x10;
            let error = graph_error(zega.import(&bytes[..]));
            assert!(
                matches!(error, Error::Checksum { .. }),
                "{tag} byte {at}: expected a checksum error, got {error}"
            );
            assert!(error.to_string().starts_with("corrupt .graph file: the "), "{error}");
            assert_eq!(export(&zega), before);
        }
    }
}

#[test]
fn a_bad_magic_is_refused() {
    let zega = Zega::in_memory().build().unwrap();
    let mut bytes = golden();
    bytes[1] = b'X';
    let error = graph_error(zega.import(&bytes[..]));
    assert!(matches!(error, Error::BadMagic));
    assert_eq!(
        error.to_string(),
        "not a .graph file: it does not start with the .graph magic bytes"
    );
    let error = graph_error(zega.import(&b"{\"nodes\": []}"[..]));
    assert!(matches!(error, Error::BadMagic));
    assert!(zega.is_empty().unwrap());
}

#[test]
fn a_newer_version_is_refused_with_an_upgrade_message() {
    let zega = Zega::in_memory().build().unwrap();
    seeded(&zega);
    let before = export(&zega);
    let mut bytes = golden();
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    let error = graph_error(zega.import(&bytes[..]));
    assert!(matches!(error, Error::NewerVersion { found: 2, supported: 1 }));
    assert_eq!(
        error.to_string(),
        "this file is .graph format version 2; this zega reads versions 1 to 1. Upgrade zega to import it"
    );
    assert_eq!(export(&zega), before);
}

#[test]
fn trailing_bytes_are_refused() {
    let zega = Zega::in_memory().build().unwrap();
    let mut bytes = golden();
    bytes.push(0);
    let error = graph_error(zega.import(&bytes[..]));
    assert!(error.to_string().contains("continues after the done section"), "{error}");
    assert!(zega.is_empty().unwrap());
}

#[test]
fn a_failed_import_on_disk_leaves_the_store_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    seeded(&zega);
    let before = export(&zega);
    let wal_before = std::fs::read(dir.path().join("wal.bin")).unwrap();
    let bytes = golden();
    for cut in cuts(&bytes) {
        graph_error(zega.import(&bytes[..cut]));
    }
    let mut corrupt = bytes.clone();
    let last = corrupt.len() - 5;
    corrupt[last] ^= 1;
    graph_error(zega.import(&corrupt[..]));
    assert_eq!(std::fs::read(dir.path().join("wal.bin")).unwrap(), wal_before);
    let staged: Vec<_> = std::fs::read_dir(dir.path().join("graphs")).unwrap().collect();
    assert!(staged.is_empty(), "a failed import left {staged:?}");
    drop(zega);
    assert_eq!(export(&open(dir.path())), before);
}

#[test]
fn an_import_on_disk_survives_reopen_and_later_writes_replay_on_top() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    seeded(&zega);
    zega.import(&golden()[..]).unwrap();
    let imported = export(&zega);
    drop(zega);

    let zega = open(dir.path());
    assert_eq!(export(&zega), imported, "the reopened store is the imported graph");
    zega.run_lang(PEOPLE, r#"mutation { Person(name: "Linus" && age: 55) { name } }"#)
        .unwrap();
    let written = zega.graph_json().unwrap();
    drop(zega);

    let zega = open(dir.path());
    let reopened = zega.graph_json().unwrap();
    assert_eq!(reopened, written);
    let people: Vec<&Json> = reopened["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["labels"][0] == "Person")
        .collect();
    assert_eq!(people.len(), 1, "only the write after the import: {people:?}");
    // The new node took the file's next id (7), not one after Ada and Grace.
    assert_eq!(people[0]["id"], 7);
}

#[test]
fn the_stored_copy_of_an_import_is_what_replay_needs() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    zega.import(&golden()[..]).unwrap();
    drop(zega);
    let copies: Vec<_> = std::fs::read_dir(dir.path().join("graphs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(std::fs::read(&copies[0]).unwrap(), golden());
    std::fs::remove_file(&copies[0]).unwrap();
    let error = Zega::open(dir.path().to_str().unwrap()).build().err().unwrap();
    assert!(error.to_string().contains("cannot be opened"), "{error}");
}

// ---------------------------------------------------------------------------
// What an import carries (schema text, declarations, metadata) is graph
// state: import then export gives the same file on every path.

#[test]
fn import_then_export_gives_back_the_same_file_in_memory_on_disk_and_through_snapshots() {
    let golden = golden();
    let zega = Zega::in_memory().build().unwrap();
    zega.import(&golden[..]).unwrap();
    assert_eq!(export(&zega), golden);

    // The explorer's localStorage path (`export_base64`) carries it too.
    let restored = Zega::in_memory().build().unwrap();
    restored.restore_bytes(&zega.snapshot_bytes().unwrap()).unwrap();
    assert_eq!(export(&restored), golden);

    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    zega.import(&golden[..]).unwrap();
    zega.snapshot().unwrap();
    drop(zega);
    assert_eq!(export(&open(dir.path())), golden);
}

#[test]
fn the_wal_is_marked_version_3_once_it_holds_an_import() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    seeded(&zega);
    let header = || std::fs::read(dir.path().join("wal.bin")).unwrap()[..6].to_vec();
    assert_eq!(header(), b"ZWAL\x02\x00");
    zega.import(&golden()[..]).unwrap();
    assert_eq!(header(), b"ZWAL\x03\x00");
    drop(zega);
    assert_eq!(export(&open(dir.path())), golden());
}

/// A graph of `n` people, loaded in one statement.
fn people(n: usize) -> Vec<u8> {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"Name":"person {i}","Age":{}}}"#, i % 90))
        .collect();
    let sources = std::collections::HashMap::from([("./p.json".to_string(), format!("[{}]", rows.join(",")))]);
    let zega = Zega::in_memory().build().unwrap();
    zega.run_lang_with_sources(PEOPLE, r#"mutation json ["./p.json"] { Person(name: $Name && age: $Age) { name } }"#, &sources)
        .unwrap();
    export(&zega)
}

fn imported_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("graphs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Opening replays from the last import only: every earlier import is
/// replaced by it, so its file is deleted as soon as the next one commits,
/// and opening after N imports reads one file, not N.
#[test]
fn reopening_after_many_imports_reads_only_the_last_one() {
    let big = people(5_000);
    let small = golden();
    let time_open = |dir: &Path| {
        let start = std::time::Instant::now();
        let zega = open(dir);
        (start.elapsed(), zega)
    };

    let one = tempfile::tempdir().unwrap();
    open(one.path()).import(&big[..]).unwrap();
    let (after_one, zega) = time_open(one.path());
    assert_eq!(export(&zega), big);
    drop(zega);

    let many = tempfile::tempdir().unwrap();
    let zega = open(many.path());
    for round in 0..30 {
        zega.import(&big[..]).unwrap();
        zega.run_lang(PEOPLE, &format!(r#"mutation {{ Person(name: "round {round}" && age: 1) {{ name }} }}"#))
            .unwrap();
        zega.import(&small[..]).unwrap();
        assert_eq!(imported_files(many.path()).len(), 1, "round {round}");
    }
    zega.import(&big[..]).unwrap();
    zega.run_lang(PEOPLE, r#"mutation { Person(name: "last" && age: 2) { name } }"#).unwrap();
    let expected = export(&zega);
    drop(zega);
    // Leftovers a crash could leave: a staging file and an unreferenced import.
    std::fs::write(many.path().join("graphs/.incoming-1-1.tmp"), b"partial").unwrap();
    std::fs::write(many.path().join("graphs").join(format!("{}.graph", "0".repeat(64))), &small).unwrap();

    let (after_many, zega) = time_open(many.path());
    assert_eq!(export(&zega), expected);
    assert_eq!(imported_files(many.path()).len(), 1, "{:?}", imported_files(many.path()));
    println!("open after 1 import: {after_one:?}; after 61 imports and 31 writes: {after_many:?}");
}

/// docs/graph-format.md shows an empty graph byte for byte; it is what this
/// zega writes (a version bump changes `created_by`, and the doc with it).
#[test]
fn the_spec_s_empty_graph_is_what_zega_writes() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/graph-format.md")).unwrap();
    let section = doc.split("## An empty graph").nth(1).unwrap();
    let block = section.split("```text").nth(1).unwrap().split("```").next().unwrap();
    let bytes: Vec<u8> = block
        .lines()
        .filter_map(|line| line.split_once(": "))
        .flat_map(|(_, rest)| {
            rest.split(' ')
                .take_while(|word| word.len() == 2 && word.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(|word| u8::from_str_radix(word, 16).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(bytes, export(&Zega::in_memory().build().unwrap()));
}

/// Two imports racing on one disk database: each commits its own file, and
/// cleanup after one never deletes the other's before its WAL entry is in.
/// The database always reopens, as one of the two graphs.
#[test]
fn concurrent_imports_leave_a_database_that_reopens() {
    let golden = golden();
    let empty = export(&Zega::in_memory().build().unwrap());
    for round in 0..40 {
        let dir = tempfile::tempdir().unwrap();
        let zega = std::sync::Arc::new(open(dir.path()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = [golden.clone(), empty.clone()]
            .into_iter()
            .map(|bytes| {
                let (zega, barrier) = (zega.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    zega.import(&bytes[..]).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let live = export(&zega);
        drop(zega);
        let reopened = Zega::open(dir.path().to_str().unwrap())
            .build()
            .unwrap_or_else(|error| panic!("round {round}: reopen failed: {error}"));
        assert_eq!(export(&reopened), live, "round {round}");
        assert!(live == golden || live == empty, "round {round}");
    }
}

/// Clearing is a durable replace-with-empty: the graph and everything an
/// import carried go, the id counters stay, and a reopen agrees.
#[test]
fn clear_drops_what_an_import_carried_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    zega.import(&golden()[..]).unwrap();
    zega.clear().unwrap();
    assert!(zega.is_empty().unwrap());
    let cleared = export(&zega);
    let summary = Zega::in_memory().build().unwrap().import(&cleared[..]).unwrap();
    assert_eq!(summary.schema, None);
    assert!(summary.meta.is_empty(), "{:?}", summary.meta);
    assert!(summary.indexes.is_empty() && summary.uniques.is_empty());
    zega.run_lang(PEOPLE, r#"mutation { Person(name: "after" && age: 1) { name } }"#).unwrap();
    // The golden graph's next node id was 7: cleared ids are not reused.
    assert_eq!(zega.graph_json().unwrap()["nodes"][0]["id"], 7);
    let written = export(&zega);
    drop(zega);
    assert_eq!(export(&open(dir.path())), written);
}

/// A database's copies of its imports are readable by its owner only.
#[cfg(unix)]
#[test]
fn imported_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let zega = open(dir.path());
    zega.import(&golden()[..]).unwrap();
    for entry in std::fs::read_dir(dir.path().join("graphs")).unwrap() {
        let mode = entry.unwrap().metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let (_, staging) = zega.staging_file("test").unwrap().unwrap();
    assert_eq!(std::fs::metadata(&staging).unwrap().permissions().mode() & 0o777, 0o600);
}
