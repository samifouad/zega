//! zega#52: checkpoints. A disk database writes its whole graph as a `.graph`
//! file and starts its WAL over from there, so the log, and what a restart
//! replays, is bounded by the writes since the last checkpoint instead of
//! every write the database ever took.
//!
//! A checkpoint is an import of the database's own graph (APS 20: "land the
//! minimal snapshot-then-truncate now"). It uses the machinery `.graph`
//! imports already have, and adds no file or WAL entry of its own:
//!
//! 1. Under the graph lock, note where the WAL ends and write the graph to a
//!    staging file in `graphs/`. Since every write appends to the WAL (and
//!    waits for it to be durable) under that same lock, the file holds
//!    exactly the log up to that byte. Writes wait for this step only.
//! 2. Without the lock: sync the file and rename it to `graphs/<sha256>.graph`,
//!    then sync the directory. Writes carry on appending to the WAL.
//! 3. [`Wal::rotate`]: write a new log holding one `ReplaceGraph` entry naming
//!    that file, followed by every entry written since step 1, sync it and
//!    rename it over `wal.bin`. That rename is the commit.
//! 4. Delete what no replay reads any more: earlier `.graph` files (imports
//!    and checkpoints) and a `snapshot.bin` from before checkpoints.
//!
//! Open already replays from the last `ReplaceGraph` entry and deletes
//! staging files and `.graph` files the log does not name, so a crash at any
//! step opens to every acknowledged write: before the rename in step 3 the
//! old log is whole, and after it the new log's file is already durable.
//! `checkpoint_tests.rs` crashes at each step.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sha2::Digest;

use crate::graph::Graph;
use crate::wal::{Operation, Wal};
use crate::{Result, ZegaError};

/// The smallest WAL a checkpoint is taken for, unless the builder says
/// otherwise: `zega start --snapshot-every-mb` (default 16).
pub const DEFAULT_SNAPSHOT_EVERY_BYTES: u64 = 16 << 20;

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
}

impl Store {
    /// Take a checkpoint now (the steps in the module comment).
    pub fn checkpoint(&self) -> Result<Checkpoint> {
        let _one_at_a_time = self
            .import_lock
            .lock()
            .map_err(|_| ZegaError::Execution("import lock poisoned".to_string()))?;
        let (file, staging) = crate::create_staging(&self.path, "checkpoint")?;
        let result = self.checkpoint_into(file, &staging);
        if result.is_err() {
            // Gone already once renamed; a leftover is removed at open.
            let _ = std::fs::remove_file(&staging);
        }
        result
    }

    fn checkpoint_into(&self, file: std::fs::File, staging: &Path) -> Result<Checkpoint> {
        let mut out = Hashing {
            inner: file,
            hash: sha2::Sha256::new(),
        };
        let (from, bytes, paused) = {
            let graph = self
                .graph
                .lock()
                .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
            let started = std::time::Instant::now();
            let from = self.wal.end()?;
            let options = crate::graph_file::ExportOptions::default();
            let summary = crate::graph_file::write(&graph, &options, crate::CREATED_BY, &mut out)?;
            (from, summary.bytes, started.elapsed())
        };
        crash_point(Step::GraphWritten);
        let Hashing { inner: file, hash } = out;
        file.sync_all()?;
        drop(file);
        crash_point(Step::GraphSynced);
        let name = format!("{}/{}.graph", crate::IMPORTS_DIR, crate::graph_file::hex(&hash.finalize()));
        let target = self.path.join(&name);
        crate::wal::rename_into_place(staging, &target)?;
        crash_point(Step::GraphRenamed);
        crate::wal::sync_parent(&target)?;
        crash_point(Step::GraphDurable);

        let wal_bytes_after = self
            .wal
            .rotate(from, &Operation::ReplaceGraph { file: name.clone() })?;
        self.base_bytes.store(bytes, Ordering::Relaxed);
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
        })
    }

    /// Whether the WAL has grown enough to checkpoint: to `floor`, and to the
    /// size of the graph it starts from, so rewriting the graph never costs
    /// more than the log it replaces (at most 2x the writes, amortised).
    /// Returns the WAL's length and that threshold.
    fn due(&self, floor: u64) -> Result<(u64, u64)> {
        let end = self.wal.end()?;
        Ok((end, floor.max(self.base_bytes.load(Ordering::Relaxed))))
    }
}

/// Feeds the SHA-256 that names the file with every byte written.
struct Hashing {
    inner: std::fs::File,
    hash: sha2::Sha256,
}

impl std::io::Write for Hashing {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        if CRASH_AT.load(Ordering::SeqCst) == Step::GraphPartial as u8 {
            // Torn: half of the first write reaches the file, then the crash.
            std::io::Write::write_all(&mut self.inner, &buf[..buf.len() / 2])?;
            crash_point(Step::GraphPartial);
        }
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
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
        let Ok((end, threshold)) = store.due(floor) else { continue };
        if end < threshold || end < retry_at {
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
    /// Part of the `.graph` file is in its staging file.
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
