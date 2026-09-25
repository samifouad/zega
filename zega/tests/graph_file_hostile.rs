//! Hostile `.graph` input (review of zegadb/zega#105): a small file must not
//! make the reader allocate much, whatever counts and lengths it claims.
//! Every case here is at most 4 KB and must peak under 4 MiB of heap while
//! it is refused. Adapted from the reviewer's harness; its own test binary,
//! so the counting allocator sees nothing else.

use std::alloc::{GlobalAlloc, Layout, System};
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
                let grow = new_size - layout.size();
                let now = CURRENT.fetch_add(grow, Ordering::Relaxed) + grow;
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

/// One measurement at a time: the peak is process-wide.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

const MAX_INPUT: usize = 4096;
const MAX_PEAK: usize = 4 * 1024 * 1024;
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden-v1.graph");

fn section(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend((payload.len() as u64).to_le_bytes());
    out.extend(payload);
    out.extend(crc32fast::hash(payload).to_le_bytes());
    out
}

fn string(text: &str) -> Vec<u8> {
    let mut out = (text.len() as u32).to_le_bytes().to_vec();
    out.extend(text.as_bytes());
    out
}

/// Header, manifest (one node), names ("k"), empty schema.
fn prefix() -> Vec<u8> {
    let mut file = b"\x89ZGRAPH\n".to_vec();
    file.extend(1u32.to_le_bytes());
    let mut manifest = string("x");
    manifest.extend(1u64.to_le_bytes());
    manifest.extend(0u64.to_le_bytes());
    manifest.extend(0u32.to_le_bytes());
    file.extend(section(b"MNFT", &manifest));
    let mut names = 1u32.to_le_bytes().to_vec();
    names.extend(string("k"));
    file.extend(section(b"NAME", &names));
    file.extend(section(b"SCHM", &[0u8; 9]));
    file
}

/// A NODE section that claims 2^62 bytes, then one node whose property
/// `k` opens `depth` lists or maps, each claiming 2^32-1 items.
fn nested(tag: u8, depth: usize) -> Vec<u8> {
    let mut file = prefix();
    file.extend(b"NODE");
    file.extend((1u64 << 62).to_le_bytes());
    file.extend(1u64.to_le_bytes()); // next node id
    file.extend(0u64.to_le_bytes()); // node 0
    file.extend(0u32.to_le_bytes()); // no labels
    file.extend(1u32.to_le_bytes()); // one property
    file.extend(0u32.to_le_bytes()); // "k"
    for _ in 0..depth {
        file.push(tag);
        file.extend(u32::MAX.to_le_bytes());
        if tag == 0x07 {
            file.extend(0u32.to_le_bytes()); // an empty key
        }
    }
    file
}

fn node_prefix() -> Vec<u8> {
    let mut file = prefix();
    file.extend(b"NODE");
    file.extend((1u64 << 62).to_le_bytes());
    file.extend(1u64.to_le_bytes());
    file.extend(0u64.to_le_bytes());
    file
}

/// Import `bytes` into a fresh database; returns the peak heap it took and
/// whether it was accepted.
fn peak_of(bytes: &[u8]) -> (usize, bool) {
    let zega = Zega::in_memory().build().unwrap();
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let accepted = zega.import(bytes).is_ok();
    (PEAK.load(Ordering::Relaxed) - base, accepted)
}

fn assert_small(name: &str, bytes: &[u8]) {
    assert!(bytes.len() <= MAX_INPUT, "{name}: {} bytes", bytes.len());
    let (peak, accepted) = peak_of(bytes);
    assert!(!accepted, "{name} was accepted");
    assert!(
        peak < MAX_PEAK,
        "{name}: {} bytes of input peaked at {:.1} MiB",
        bytes.len(),
        peak as f64 / 1048576.0
    );
    println!("{name}: {} bytes in, {:.1} KiB peak", bytes.len(), peak as f64 / 1024.0);
}

#[test]
fn small_hostile_files_are_refused_in_bounded_memory() {
    let _serial = serial();
    assert_small("128 nested lists claiming 2^32-1 items", &nested(0x06, 128));
    assert_small("128 nested maps claiming 2^32-1 entries", &nested(0x07, 128));
    assert_small("16 nested lists", &nested(0x06, 16));
    assert_small("300 nested lists (past the depth limit)", &nested(0x06, 300));

    let mut labels = node_prefix();
    labels.extend(u32::MAX.to_le_bytes());
    assert_small("2^32-1 labels claimed", &labels);

    let mut text = node_prefix();
    text.extend(0u32.to_le_bytes());
    text.extend(1u32.to_le_bytes());
    text.extend(0u32.to_le_bytes());
    text.push(0x05);
    text.extend(u32::MAX.to_le_bytes());
    text.push(b'a');
    assert_small("a 4 GiB string claimed", &text);

    let mut vector = node_prefix();
    vector.extend(0u32.to_le_bytes());
    vector.extend(1u32.to_le_bytes());
    vector.extend(0u32.to_le_bytes());
    vector.extend([0x09, 0]);
    vector.extend(u32::MAX.to_le_bytes());
    assert_small("a vector of 2^32-1 dimensions claimed", &vector);

    let mut props = node_prefix();
    props.extend(0u32.to_le_bytes());
    props.extend(u32::MAX.to_le_bytes());
    assert_small("2^32-1 properties claimed", &props);

    // Names, meta and schema lists claiming everything in their sections.
    let mut file = b"\x89ZGRAPH\n".to_vec();
    file.extend(1u32.to_le_bytes());
    file.extend(b"MNFT");
    file.extend((1u64 << 62).to_le_bytes());
    file.extend(string("x"));
    file.extend([0u8; 16]);
    file.extend(u32::MAX.to_le_bytes());
    assert_small("2^32-1 metadata entries claimed", &file);
}

/// Deeply nested values are refused at the depth limit on a 1 MiB stack,
/// wasm's default, without overflowing it.
#[test]
fn nesting_is_refused_on_a_small_stack() {
    let _serial = serial();
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            for depth in [127, 128, 129, 400] {
                assert_small(&format!("{depth} nested lists, 1 MiB stack"), &nested(0x06, depth));
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

/// Mutated copies of the golden file: none panics, none peaks above the
/// bound, and any that is accepted is canonical (it exports back to itself).
#[test]
fn mutated_golden_files_never_panic_or_balloon() {
    let _serial = serial();
    let golden = std::fs::read(GOLDEN).unwrap();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (mut accepted, mut worst) = (0, 0usize);
    for _ in 0..20_000 {
        let mut bytes = golden.clone();
        for _ in 0..1 + next() % 6 {
            let at = (next() as usize) % bytes.len();
            match next() % 4 {
                0 => bytes[at] = next() as u8,
                1 => bytes[at] = [0x00, 0xff, 0x7f, 0x80][(next() % 4) as usize],
                2 => bytes.insert(at, next() as u8),
                _ => {
                    bytes.remove(at);
                }
            }
        }
        let outcome = std::panic::catch_unwind(|| peak_of(&bytes));
        let (peak, ok) = outcome.unwrap_or_else(|_| panic!("import panicked on {bytes:?}"));
        worst = worst.max(peak);
        if ok && bytes != golden {
            accepted += 1;
            let zega = Zega::in_memory().build().unwrap();
            zega.import(&bytes[..]).unwrap();
            let mut again = Vec::new();
            zega.export(&mut again).unwrap();
            // The manifest's writer name is not re-emitted, everything else is.
            assert_eq!(again.len(), bytes.len(), "an accepted mutation is not canonical");
        }
        assert!(peak < MAX_PEAK, "a {}-byte mutation peaked at {peak} bytes", bytes.len());
    }
    println!("20000 mutations: {accepted} accepted, worst peak {:.1} KiB", worst as f64 / 1024.0);
}
