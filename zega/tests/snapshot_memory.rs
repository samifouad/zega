//! A restore holds the graph once (zegadb/zega#100): each snapshot record
//! goes into the new graph as it is decoded, so opening a data directory
//! needs little beyond the graph it builds. Before, the whole snapshot was
//! decoded into maps of records first, and a restart's peak (which is what
//! RSS keeps) was the graph plus that copy.
//!
//! Heap is measured with a counting global allocator; this file is its own
//! test binary with one test, so nothing else allocates while it measures.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

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

fn reset_peak() -> usize {
    let now = current();
    PEAK.store(now, Ordering::Relaxed);
    now
}

fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

const NODES: usize = 100_000;
const SCHEMA: &str = "schema { type Item { n: Int name: String city: String score: Int \
     at: Point from (lat, lon) next -> Item[] } } unique { Item { n } }";

#[test]
fn restoring_a_snapshot_needs_little_beyond_the_graph() {
    // The Fly benchmark's shape: five fields and three links per node,
    // loaded as it loaded them, in CSV batches (a load is capped at 2 MB).
    let source = Zega::in_memory().build().unwrap();
    let load = |statement: &str, header: &str, rows: &mut dyn Iterator<Item = String>| loop {
        let batch: Vec<String> = rows.take(10_000).collect();
        if batch.is_empty() {
            break;
        }
        let csv = format!("{header}\n{}\n", batch.join("\n"));
        let sources = HashMap::from([("rows.csv".to_string(), csv)]);
        source
            .apply_zql_with_sources(&format!("{SCHEMA}\n{statement}"), &sources)
            .unwrap();
    };
    load(
        "mutation csv [\"rows.csv\"] { Item(n: $n && name: $name && city: $city && score: $score) { n } }",
        "n,name,city,score,lat,lon",
        &mut (0..NODES).map(|n| {
            let (lat, lon) = ((n % 170) as f64 - 85.0, (n % 358) as f64 - 179.0);
            format!("{n},item-{n},c{},{},{lat},{lon}", n % 500, n % 1000)
        }),
    );
    load(
        "mutation csv [\"rows.csv\"] { Item(n: $from) { next -> link Item(n: $to) { n } } }",
        "from,to",
        &mut (0..NODES * 3).map(|i| format!("{},{}", i / 3, (i * 7 + 13) % NODES)),
    );
    let bytes = source.snapshot_bytes().unwrap();
    drop(source);

    let target = Zega::in_memory().build().unwrap();
    let base = reset_peak();
    target.restore_bytes(&bytes).unwrap();
    let graph = current() - base;
    let extra = peak() - base - graph;
    println!(
        "{NODES} nodes: graph heap {:.1} MiB; the restore peaked {:.1} MiB above it ({:.2}x the graph)",
        graph as f64 / 1048576.0,
        extra as f64 / 1048576.0,
        (graph + extra) as f64 / graph as f64,
    );
    // The graph really is back.
    assert_eq!(
        target.run_lang(SCHEMA, "{ Item(n: 4242) { name next -> Item { n } } }").unwrap()["name"],
        "item-4242"
    );
    // Decoding into maps of records first needed a third of the graph again.
    assert!(
        extra * 10 <= graph,
        "a restore needed {extra} bytes beyond the {graph}-byte graph; the bound is a tenth of it"
    );
}
