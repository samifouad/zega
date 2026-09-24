//! Copied from the zega-cloud spike (codex/cloud-spike): live and peak Rust
//! heap, plus WASM linear-memory capacity. Isolate-wide, not per object.
//! Requested live/peak Rust allocation bytes, plus WASM linear-memory capacity.
//! These are isolate-wide metrics, not V8 heap or per-object attribution.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
struct Measured;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            add(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = System.realloc(ptr, layout, size);
        if !next.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            add(size);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Measured = Measured;
pub fn live() -> usize {
    LIVE.load(Relaxed)
}
pub fn peak() -> usize {
    PEAK.load(Relaxed)
}
pub fn capacity() -> usize {
    core::arch::wasm32::memory_size(0) * 65536
}
