//! zega#52: checkpoints. A disk database writes its whole graph as a `.graph`
//! file and starts its WAL over from there, so the log, and what a restart
//! replays, is bounded by the writes since the last checkpoint instead of
//! every write the database ever took.
//!
//! A checkpoint is an import of the database's own graph (APS 20: "land the
//! minimal snapshot-then-truncate now"). It uses the machinery `.graph`
//! imports already have, and adds no file or WAL entry of its own:
//!
//! 1. Under the graph lock, note where the WAL ends and encode the graph as a
//!    `.graph` file in one pass (`graph_file::write_unfinished`), into memory
//!    up to [`MEMORY_CAP`] and into its staging file in `graphs/` beyond it.
//!    Since every write appends to the WAL (and waits for it to be durable)
//!    under that same lock, the file holds exactly the log up to that byte.
//!    Everything that takes the lock waits for this step: writes, and reads
//!    too, since the graph has one lock. The pause grows with the graph.
//! 2. Without the lock: write what is in memory to the staging file, read it back for its digests, add its last
//!    section, sync it and rename it to `graphs/<sha256>.graph`, then sync
//!    the directory. Reads and writes carry on.
//! 3. [`Wal::rotate`]: write a new log holding one `ReplaceGraph` entry naming
//!    that file, followed by every entry written since step 1, sync it and
//!    rename it over `wal.bin`. That rename is the commit.
//! 4. Delete what no replay reads any more: earlier `.graph` files (imports
//!    and checkpoints) and a `snapshot.bin` from before checkpoints.
//!
//! Open replays from the last `ReplaceGraph` entry, so a crash at any step
//! opens to every acknowledged write: before the rename in step 3 the old log
//! is whole, and after it the new log's file is already durable. Open deletes
//! staging files, and `.graph` files only when the log names another one; it
//! refuses to open a log with no entries beside a `.graph` file (the log that
//! named it is lost) and promotes a whole `wal.rotate.tmp` found without a
//! log. `checkpoint_tests.rs` crashes at each step and deletes, empties and
//! renames the log.
//!
//! The first checkpoint marks the WAL version 3, as the first import does: a
//! zega from before `.graph` imports refuses the directory after it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::graph::Graph;
use crate::wal::{Operation, Wal};
use crate::{Result, ZegaError};

/// The smallest WAL a checkpoint is taken for, unless the builder says
/// otherwise: `zega start --snapshot-every-mb` (default 16).
pub const DEFAULT_SNAPSHOT_EVERY_BYTES: u64 = 16 << 20;

/// The most of a checkpoint's `.graph` file held in memory while the graph
/// lock is held. Encoding into memory keeps the lock for the encoding alone
/// (about half the pause of writing the file under it); past this size the
/// rest goes straight to the staging file, so the extra memory stays bounded
/// whatever the graph (Pro graphs are capped at 64 MiB).
pub const MEMORY_CAP: u64 = 256 << 20;

/// How often the checkpoint thread looks at the WAL's length.
const POLL: Duration = Duration::from_millis(100);

/// What a checkpoint wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    /// The `.graph` file, relative to the data directory (`graphs/<sha>.graph`).
    pub file: String,
    /// That file's size.
    pub graph_bytes: u64,
    /// The WAL's length before the checkpoint, and after it.
    pub wal_bytes_before: u64,
    pub wal_bytes_after: u64,
    /// How long writes waited: the graph lock was held this long.
    pub paused: Duration,
    /// Whether the file outgrew [`MEMORY_CAP`] and was written to disk under
    /// the lock from there on.
    pub spilled: bool,
}

/// What a checkpoint needs of a database, shared with its checkpoint thread.
#[derive(Clone)]
pub(crate) struct Store {
    pub graph: Arc<Mutex<Graph>>,
    pub wal: Arc<Wal>,
    pub path: PathBuf,
    /// Held by an import from its rename into `graphs/` through its WAL entry
    /// and cleanup, and by a checkpoint from start to finish: each deletes
    /// the other's superseded files, so they never overlap.
    pub import_lock: Arc<Mutex<()>>,
    /// The size of the `.graph` file the WAL starts from, 0 without one.
    pub base_bytes: Arc<AtomicU64>,
    pub counts: Arc<Counts>,
}

/// How many checkpoints a database has taken and failed since it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckpointCounts {
    pub taken: u64,
    pub failed: u64,
}

#[derive(Default)]
pub(crate) struct Counts {
    taken: AtomicU64,
    failed: AtomicU64,
    /// The WAL's length when the last checkpoint finished: what it had
    /// already outgrown then (writes made during that checkpoint) is not
    /// counted again, so checkpoints never run back to back.
    wal_after: AtomicU64,
}

impl Counts {
    pub fn get(&self) -> CheckpointCounts {
        CheckpointCounts {
            taken: self.taken.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

impl Store {
    /// Take a checkpoint now (the steps in the module comment).
    pub fn checkpoint(&self) -> Result<Checkpoint> {
        let _one_at_a_time = self
            .import_lock
            .lock()
            .map_err(|_| ZegaError::Execution("import lock poisoned".to_string()))?;
        let result = crate::create_staging(&self.path, "checkpoint").and_then(|(file, staging)| {
            let result = self.checkpoint_into(file, &staging);
            if result.is_err() {
                // Gone already once renamed; a leftover is removed at open.
                let _ = std::fs::remove_file(&staging);
            }
            result
        });
        let counter = if result.is_ok() { &self.counts.taken } else { &self.counts.failed };
        counter.fetch_add(1, Ordering::Relaxed);
        result
    }

    fn checkpoint_into(&self, mut file: std::fs::File, staging: &Path) -> Result<Checkpoint> {
        let (from, unfinished, paused) = {
            let graph = self
                .graph
                .lock()
                .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
            let started = std::time::Instant::now();
            let from = self.wal.end()?;
            let mut out = Spill {
                memory: Some(std::io::Cursor::new(Vec::new())),
                file: &mut file,
                cap: memory_cap(),
            };
            let unfinished = crate::graph_file::write_unfinished(&graph, crate::CREATED_BY, &mut out)?;
            let paused = started.elapsed();
            drop(graph);
            let spilled = out.memory.is_none();
            out.spill()?;
            (from, (unfinished, spilled), paused)
        };
        let (unfinished, spilled) = unfinished;
        crash_point(Step::GraphPartial);
        let (summary, sha256) = crate::graph_file::finish(&mut file, &unfinished)?;
        let bytes = summary.bytes;
        crash_point(Step::GraphWritten);
        file.sync_all()?;
        drop(file);
        crash_point(Step::GraphSynced);
        let name = format!("{}/{}.graph", crate::IMPORTS_DIR, crate::graph_file::hex(&sha256));
        let target = self.path.join(&name);
        crate::wal::rename_into_place(staging, &target)?;
        crash_point(Step::GraphRenamed);
        crate::wal::sync_parent(&target)?;
        crash_point(Step::GraphDurable);

        let wal_bytes_after = self
            .wal
            .rotate(from, &Operation::ReplaceGraph { file: name.clone() })?;
        self.base_bytes.store(bytes, Ordering::Relaxed);
        self.counts.wal_after.store(wal_bytes_after, Ordering::Relaxed);
        // Committed. What is left is garbage a failed delete only leaves for
        // the next open, never an error.
        let _ = crate::remove_imported_except(&self.path, &name);
        crash_point(Step::ImportsRemoved);
        let _ = std::fs::remove_file(self.path.join(crate::SNAPSHOT_FILE));
        Ok(Checkpoint {
            file: name,
            graph_bytes: bytes,
            wal_bytes_before: from,
            wal_bytes_after,
            paused,
            spilled,
        })
    }

    /// Whether the WAL has grown enough since the last checkpoint to take
    /// another: by `floor`, and by the size of the graph it starts from, so
    /// rewriting the graph never costs more than the log it replaces (at most
    /// 2x the writes, amortised). Returns the WAL's length, whether it is
    /// due, and that threshold.
    pub(crate) fn due(&self, floor: u64) -> Result<(u64, bool, u64)> {
        let end = self.wal.end()?;
        let threshold = floor.max(self.base_bytes.load(Ordering::Relaxed));
        let grown = end.saturating_sub(self.counts.wal_after.load(Ordering::Relaxed));
        Ok((end, grown >= threshold, threshold))
    }
}

/// A seekable file that stays in memory until it would pass `cap` bytes,
/// then moves to `file` and continues there.
struct Spill<'a> {
    memory: Option<std::io::Cursor<Vec<u8>>>,
    file: &'a mut std::fs::File,
    cap: u64,
}

impl Spill<'_> {
    /// Move what is in memory to the file, at the same position.
    fn spill(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, Write};
        if let Some(memory) = self.memory.take() {
            self.file.write_all(memory.get_ref())?;
            self.file.seek(std::io::SeekFrom::Start(memory.position()))?;
        }
        Ok(())
    }
}

impl std::io::Write for Spill<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(memory) = &mut self.memory {
            let end = (memory.position() + buf.len() as u64).max(memory.get_ref().len() as u64);
            if end <= self.cap {
                return memory.write(buf);
            }
            self.spill()?;
        }
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.memory {
            Some(_) => Ok(()),
            None => self.file.flush(),
        }
    }
}

impl std::io::Seek for Spill<'_> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        match &mut self.memory {
            Some(memory) => memory.seek(to),
            None => self.file.seek(to),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// A test's [`MEMORY_CAP`] for checkpoints taken on this thread.
    pub(crate) static MEMORY_CAP_FOR_TEST: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn memory_cap() -> u64 {
    #[cfg(test)]
    if let Some(cap) = MEMORY_CAP_FOR_TEST.with(|cap| cap.get()) {
        return cap;
    }
    MEMORY_CAP
}

/// A thread that checkpoints once the WAL is due ([`Store::due`]). Dropping
/// it stops the thread, after the checkpoint it may be taking.
pub(crate) struct Checkpointer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Checkpointer {
    pub fn spawn(store: Store, floor: u64) -> std::io::Result<Self> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("zega-checkpoint".to_string())
            .spawn(move || run(store, floor, &signal))?;
        Ok(Checkpointer {
            stop,
            thread: Some(thread),
        })
    }
}

fn run(store: Store, floor: u64, stop: &(Mutex<bool>, Condvar)) {
    // After a failure, wait for another `threshold` bytes before retrying,
    // rather than rewriting the graph on every poll.
    let mut retry_at = 0u64;
    loop {
        {
            let Ok(stopped) = stop.0.lock() else { return };
            let Ok((stopped, _)) = stop.1.wait_timeout_while(stopped, POLL, |stopped| !*stopped) else {
                return;
            };
            if *stopped {
                return;
            }
        }
        // A poisoned WAL refuses writes, so it cannot grow: nothing to do.
        let Ok((end, due, threshold)) = store.due(floor) else { continue };
        if !due || end < retry_at {
            continue;
        }
        if let Err(error) = store.checkpoint() {
            retry_at = end.saturating_add(threshold);
            eprintln!(
                "zega: checkpoint of {} failed, the WAL keeps every write and it is retried \
                 after {threshold} more bytes: {error}",
                store.path.display()
            );
        }
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        if let Ok(mut stopped) = self.stop.0.lock() {
            *stopped = true;
            self.stop.1.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The steps of a checkpoint a test can crash at ([`crash_point`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Step {
    /// The `.graph` file is in its staging file but for its last section.
    GraphPartial = 1,
    /// All of it, not synced.
    GraphWritten,
    /// Synced, not renamed into `graphs/`.
    GraphSynced,
    /// Renamed; the directory is not synced.
    GraphRenamed,
    /// The rename is durable; the WAL is untouched.
    GraphDurable,
    /// Part of the next WAL is in `wal.rotate.tmp`.
    WalNextPartial,
    /// All of it, not synced.
    WalNextWritten,
    /// Synced, not renamed over `wal.bin`.
    WalNextSynced,
    /// Renamed; the directory is not synced.
    WalRenamed,
    /// The rename is durable; nothing superseded is deleted yet.
    WalDurable,
    /// Earlier `.graph` files are deleted; a `snapshot.bin` is not.
    ImportsRemoved,
}

#[cfg(test)]
pub(crate) static CRASH_AT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// A test's hook on the steps of a checkpoint.
#[cfg(test)]
pub(crate) type StepHook = Box<dyn FnMut(Step)>;

#[cfg(test)]
thread_local! {
    /// Runs at every step of a checkpoint taken on this thread.
    pub(crate) static AT_STEP: std::cell::RefCell<Option<StepHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Abort the process at `step` when a test asked for it: what a crash (or a
/// `kill -9`) there leaves on disk. Nothing outside tests.
#[inline]
pub(crate) fn crash_point(step: Step) {
    #[cfg(test)]
    {
        if CRASH_AT.load(Ordering::SeqCst) == step as u8 {
            std::process::abort();
        }
        AT_STEP.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook(step);
            }
        });
    }
    #[cfg(not(test))]
    let _ = step;
}
