//! Export streams: exporting a million-node graph needs a bounded amount of
//! memory beyond the graph itself, where the JSON `GET /graph` used to
//! need more than the graph again (zegadb/zega#52: 2.3x RSS).
//!
//! Heap is measured with a counting global allocator. That is the part of
//! RSS an export can grow, and unlike RSS it is exact and the same on every
//! OS CI runs. This file is its own test binary with one test, so nothing
//! else allocates while it measures.
//!
//! The graph is fed to `Zega::import` from a hand-written `.graph` stream
//! (an independent second writer, built from docs/graph-format.md), so the
//! test also shows import streams: no file or section is ever in memory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};
use zega::Zega;

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size >= layout.size() {
                let now = CURRENT.fetch_add(new_size - layout.size(), Ordering::Relaxed)
                    + (new_size - layout.size());
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

/// Start a new peak measurement from what is allocated now.
fn reset_peak() -> usize {
    let now = current();
    PEAK.store(now, Ordering::Relaxed);
    now
}

fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

const NODES: u64 = 1_000_000;
/// The JSON comparison needs the graph and 1.6-2.2x more again, so it runs
/// on a smaller graph to keep the test inside a 7 GB CI runner (at 1M nodes
/// it peaks at 2.6x the graph's heap, 4.5 GB).
const JSON_NODES: u64 = 100_000;
const MIB: f64 = 1024.0 * 1024.0;

/// Export may use this much heap beyond the graph: the name dictionary, a
/// 64 KiB write buffer and per-record scratch. It does not grow with the
/// graph; a copy of the graph, the file or a section would blow it.
const EXPORT_BOUND: usize = 4 * 1024 * 1024;

/// A second `.graph` writer, from the spec: frames sections, checksums them
/// and hashes the content sections as bytes go out.
struct Out<W: Write> {
    out: std::io::BufWriter<W>,
    content: Option<Sha256>,
    crc: crc32fast::Hasher,
    expected: u64,
    written: u64,
}

impl<W: Write> Out<W> {
    fn raw(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(content) = &mut self.content {
            content.update(bytes);
        }
        self.out.write_all(bytes)
    }
    fn begin(&mut self, tag: &[u8; 4], len: u64) -> std::io::Result<()> {
        self.raw(tag)?;
        self.raw(&len.to_le_bytes())?;
        self.crc = crc32fast::Hasher::new();
        (self.expected, self.written) = (len, 0);
        Ok(())
    }
    fn put(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.crc.update(bytes);
        self.written += bytes.len() as u64;
        self.raw(bytes)
    }
    fn end(&mut self) -> std::io::Result<()> {
        assert_eq!(self.written, self.expected);
        let crc = std::mem::take(&mut self.crc).finalize();
        self.raw(&crc.to_le_bytes())
    }
}

fn string(text: &str) -> Vec<u8> {
    [&(text.len() as u32).to_le_bytes()[..], text.as_bytes()].concat()
}

/// Writes the test graph as `.graph` bytes, record by record: node `i` is a
/// `City` with a fixed-width `name` and a `pop`, and `ROUTE` joins `i` to
/// `i + 1`. Fixed-width records make every section's length known up front.
fn write_graph(nodes: u64, out: impl Write) -> std::io::Result<()> {
    let rels = nodes - 1;
    let mut out = Out {
        out: std::io::BufWriter::new(out),
        content: None,
        crc: crc32fast::Hasher::new(),
        expected: 0,
        written: 0,
    };
    out.raw(b"\x89ZGRAPH\n")?;
    out.raw(&1u32.to_le_bytes())?;

    let created_by = string("memory test");
    out.begin(b"MNFT", created_by.len() as u64 + 20)?;
    out.put(&created_by)?;
    for count in [nodes, rels] {
        out.put(&count.to_le_bytes())?;
    }
    out.put(&0u32.to_le_bytes())?;
    out.end()?;

    out.content = Some(Sha256::new());
    // Sorted by bytes: City=0, ROUTE=1, name=2, pop=3.
    let names: Vec<u8> = ["City", "ROUTE", "name", "pop"].iter().flat_map(|name| string(name)).collect();
    out.begin(b"NAME", 4 + names.len() as u64)?;
    out.put(&4u32.to_le_bytes())?;
    out.put(&names)?;
    out.end()?;

    // No schema text, no index, no unique.
    out.begin(b"SCHM", 9)?;
    out.put(&[0u8; 9])?;
    out.end()?;

    // id, 1 label (City), 2 props: name = "c0000001" (tag 5), pop = id (tag 3).
    out.begin(b"NODE", 8 + nodes * 50)?;
    out.put(&(nodes + 1).to_le_bytes())?; // next node id
    for id in 1..=nodes {
        out.put(&id.to_le_bytes())?;
        out.put(&1u32.to_le_bytes())?;
        out.put(&0u32.to_le_bytes())?;
        out.put(&2u32.to_le_bytes())?;
        out.put(&2u32.to_le_bytes())?;
        out.put(&[0x05])?;
        out.put(&string(&format!("c{id:07}")))?;
        out.put(&3u32.to_le_bytes())?;
        out.put(&[0x03])?;
        out.put(&(id as i64).to_le_bytes())?;
    }
    out.end()?;

    // id, kind ROUTE, from id, to id + 1, no props.
    out.begin(b"RELS", 8 + rels * 32)?;
    out.put(&(rels + 1).to_le_bytes())?; // next relationship id
    for id in 1..=rels {
        out.put(&id.to_le_bytes())?;
        out.put(&1u32.to_le_bytes())?;
        out.put(&id.to_le_bytes())?;
        out.put(&(id + 1).to_le_bytes())?;
        out.put(&0u32.to_le_bytes())?;
    }
    out.end()?;

    let digest = out.content.take().unwrap().finalize();
    out.begin(b"DONE", 32)?;
    out.put(&digest)?;
    out.end()?;
    out.out.flush()
}

/// Import `nodes` nodes from a streamed file; returns the database and the
/// heap its graph holds.
fn load(nodes: u64) -> (Zega, usize) {
    let zega = Zega::in_memory().build().unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    let base = reset_peak();
    let producer = std::thread::spawn(move || write_graph(nodes, writer));
    let summary = zega.import(reader).unwrap();
    producer.join().unwrap().unwrap();
    assert_eq!((summary.nodes, summary.relationships), (nodes, nodes - 1));
    let graph = current() - base;
    println!(
        "{nodes} nodes, {} relationships: graph heap {:.1} MiB, import peak {:.3}x the graph",
        nodes - 1,
        graph as f64 / MIB,
        (peak() - base) as f64 / graph as f64
    );
    (zega, graph)
}

/// Heap an export needs beyond the graph, and the bytes it wrote.
fn export_extra(zega: &Zega) -> (usize, u64) {
    let before = reset_peak();
    let mut sink = CountingSink(0);
    let summary = zega.export(&mut sink).unwrap();
    assert_eq!(summary.bytes, sink.0);
    (peak() - before, summary.bytes)
}

#[test]
fn exporting_a_million_nodes_needs_bounded_memory_beyond_the_graph() {
    // What `GET /graph` did before .graph, against what it does now.
    let (zega, graph) = load(JSON_NODES);
    let before = reset_peak();
    let json = zega.graph_json().unwrap();
    let body = serde_json::to_vec(&serde_json::json!({ "ok": true, "result": json })).unwrap();
    let json_extra = peak() - before;
    drop((json, body));
    let (extra, _) = export_extra(&zega);
    println!(
        "  JSON GET /graph peak {:.2}x the graph; .graph export peak {:.4}x",
        (graph + json_extra) as f64 / graph as f64,
        (graph + extra) as f64 / graph as f64
    );
    drop(zega);

    let (zega, graph) = load(NODES);
    let (extra, bytes) = export_extra(&zega);
    println!(
        "  .graph export: {:.3} MiB beyond the graph (peak {:.4}x), {:.1} MiB written",
        extra as f64 / MIB,
        (graph + extra) as f64 / graph as f64,
        bytes as f64 / MIB
    );
    assert!(
        extra <= EXPORT_BOUND,
        "export needed {extra} bytes beyond the graph; the bound is {EXPORT_BOUND}"
    );
}

struct CountingSink(u64);

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
