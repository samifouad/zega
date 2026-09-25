pub mod location;
pub mod vector;
mod vector_view;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use crate::graph::Graph;
pub use crate::value::Value;
#[cfg(not(target_arch = "wasm32"))]
use crate::wal::restore;
#[cfg(not(target_arch = "wasm32"))]
use crate::wal::Operation;
use crate::wal::Wal;

#[cfg(not(target_arch = "wasm32"))]
mod checkpoint;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod checkpoint_tests;
mod graph;
pub mod graph_file;
mod index;
mod journal;
mod lang;
mod path;
mod text_fold;
mod v2;
mod validation;
mod value;
mod wal;
#[cfg(test)]
mod wal_order_tests;

#[cfg(not(target_arch = "wasm32"))]
pub use crate::checkpoint::{Checkpoint, CheckpointCounts, DEFAULT_SNAPSHOT_EVERY_BYTES};
pub use crate::lang::{diagnose, fmt};
pub use crate::validation::{Diagnostic, Pane, Report, Severity};

pub use v2::{check_zql, parse_import, zql_load_locations, ZqlEntryPoint};
pub use lang::{Direction as SchemaDirection, DisplayConfig, DisplayView, GlobeCamera, GlobeCenter, NodeDisplay, NodeShape, EdgeField, Field, LoadFormat, Schema, Span, TypeDef, ViewKind};

#[derive(Error, Debug)]
pub enum ZegaError {
    #[error("wal error: {0}")]
    Wal(#[from] crate::wal::WalError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("execution error: {0}")]
    Execution(String),
    /// A ZQL statement ran past the time limit set with
    /// [`ZegaBuilder::query_time_limit`]. It stopped where it was; a
    /// mutation's writes were rolled back.
    #[error("query exceeded the {} limit", seconds(*limit))]
    QueryTimeLimit { limit: std::time::Duration },
    /// A `.graph` file could not be written or read (docs/graph-format.md).
    /// On import, nothing was changed.
    #[error(transparent)]
    GraphFile(#[from] crate::graph_file::Error),
}

/// Who wrote a `.graph` file, as its manifest records it.
pub const CREATED_BY: &str = concat!("zega ", env!("CARGO_PKG_VERSION"));

/// `2 s`, `1.5 s`, `0.05 s`: a limit as a person would write it.
fn seconds(limit: std::time::Duration) -> String {
    let text = format!("{:.3}", limit.as_secs_f64());
    format!("{} s", text.trim_end_matches('0').trim_end_matches('.'))
}

pub type Result<T> = std::result::Result<T, ZegaError>;

pub struct Zega {
    graph: Arc<Mutex<Graph>>,
    wal: Arc<Wal>,
    #[cfg(not(target_arch = "wasm32"))]
    path: PathBuf,
    in_memory: bool,
    traversal_work_budget: usize,
    query_time_limit: Option<std::time::Duration>,
    allow_private_imports: bool,
    /// Held from an import's rename into `graphs/` through its WAL entry
    /// and the cleanup of earlier imports, so two concurrent imports never
    /// delete each other's files.
    #[cfg(not(target_arch = "wasm32"))]
    import_lock: Arc<Mutex<()>>,
    /// The size of the `.graph` file the WAL starts from (0 without one):
    /// the WAL must outgrow it before the next checkpoint.
    #[cfg(not(target_arch = "wasm32"))]
    base_bytes: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(not(target_arch = "wasm32"))]
    checkpoint_counts: Arc<checkpoint::Counts>,
    /// Takes a checkpoint whenever the WAL is due; `None` in memory or when
    /// the builder turned automatic checkpoints off.
    #[cfg(not(target_arch = "wasm32"))]
    checkpointer: Option<checkpoint::Checkpointer>,
}

pub struct ZegaBuilder {
    path: PathBuf,
    in_memory: bool,
    wal_flush_every: bool,
    #[cfg(not(target_arch = "wasm32"))]
    wal_flush_interval_ms: Option<u64>,
    traversal_work_budget: usize,
    query_time_limit: Option<std::time::Duration>,
    allow_private_imports: bool,
    #[cfg(not(target_arch = "wasm32"))]
    snapshot_every: u64,
}

const DEFAULT_TRAVERSAL_WORK_BUDGET: usize = 1_000_000;

impl ZegaBuilder {
    pub fn wal_flush_every_write(self) -> Self {
        ZegaBuilder {
            wal_flush_every: true,
            ..self
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn wal_flush_interval(self, ms: u64) -> Self {
        ZegaBuilder {
            wal_flush_interval_ms: Some(ms),
            ..self
        }
    }

    pub fn traversal_work_budget(mut self, max_relationships: usize) -> Self {
        self.traversal_work_budget = max_relationships;
        self
    }

    /// Stop any one ZQL statement (`run_lang`, or each statement `apply_zql`
    /// runs) that is still working after `limit`, with
    /// [`ZegaError::QueryTimeLimit`]; a mutation's writes are rolled back.
    /// Off by default, so an embedded or in-browser database has no limit
    /// unless its host sets one. `zega start` sets two seconds.
    pub fn query_time_limit(mut self, limit: std::time::Duration) -> Self {
        self.query_time_limit = Some(limit);
        self
    }

    /// Permit HTTP imports from private/loopback hosts. Off by default; only
    /// enable for trusted ZQL callers that may access this machine's network.
    pub fn allow_private_imports(mut self, allow: bool) -> Self {
        self.allow_private_imports = allow;
        self
    }

    /// Checkpoint a disk database ([`Zega::snapshot`]) on its own once its
    /// WAL reaches `bytes`, and the size of the graph it starts from. 0 turns
    /// automatic checkpoints off. Default [`DEFAULT_SNAPSHOT_EVERY_BYTES`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn snapshot_every(mut self, bytes: u64) -> Self {
        self.snapshot_every = bytes;
        self
    }

    pub fn build(self) -> Result<Zega> {
        Zega::open_with_builder(self)
    }
}

impl Zega {
    pub fn open(path: &str) -> ZegaBuilder {
        ZegaBuilder {
            path: PathBuf::from(path),
            in_memory: false,
            wal_flush_every: false,
            #[cfg(not(target_arch = "wasm32"))]
            wal_flush_interval_ms: None,
            traversal_work_budget: DEFAULT_TRAVERSAL_WORK_BUDGET,
            query_time_limit: None,
            allow_private_imports: false,
            #[cfg(not(target_arch = "wasm32"))]
            snapshot_every: DEFAULT_SNAPSHOT_EVERY_BYTES,
        }
    }

    pub fn in_memory() -> ZegaBuilder {
        ZegaBuilder {
            path: PathBuf::from(":memory:"),
            in_memory: true,
            wal_flush_every: false,
            #[cfg(not(target_arch = "wasm32"))]
            wal_flush_interval_ms: None,
            traversal_work_budget: DEFAULT_TRAVERSAL_WORK_BUDGET,
            query_time_limit: None,
            allow_private_imports: false,
            #[cfg(not(target_arch = "wasm32"))]
            snapshot_every: DEFAULT_SNAPSHOT_EVERY_BYTES,
        }
    }

    fn open_with_builder(builder: ZegaBuilder) -> Result<Zega> {
        let path = builder.path;
        let traversal_work_budget = builder.traversal_work_budget;
        #[cfg(not(target_arch = "wasm32"))]
        if !builder.in_memory {
            std::fs::create_dir_all(&path)?;
        }

        #[cfg(not(target_arch = "wasm32"))]
        let mut graph = Graph::new();
        #[cfg(target_arch = "wasm32")]
        let graph = Graph::new();

        #[cfg(not(target_arch = "wasm32"))]
        let snapshot_path = path.join(SNAPSHOT_FILE);
        let wal_path = path.join("wal.bin");

        // A rotation a crash interrupted is settled before the log is opened.
        #[cfg(not(target_arch = "wasm32"))]
        if !builder.in_memory {
            crate::wal::settle_rotation(&wal_path)?;
        }

        // Replay WAL
        #[cfg(not(target_arch = "wasm32"))]
        let wal = if builder.in_memory {
            Wal::in_memory()
        } else {
            Wal::with_group_commit(
                &wal_path,
                builder.wal_flush_every,
                std::time::Duration::from_millis(builder.wal_flush_interval_ms.unwrap_or(5)),
                64,
            )?
        };
        #[cfg(target_arch = "wasm32")]
        let wal = Wal::in_memory();
        #[cfg(not(target_arch = "wasm32"))]
        let mut base_bytes = 0;
        #[cfg(not(target_arch = "wasm32"))]
        if !builder.in_memory {
            let ops = if wal_path.exists() { wal.iter()? } else { Vec::new() };
            // The last import replaces everything before it, snapshot
            // included: replay starts there, and reads only that one file.
            let last_import = ops
                .iter()
                .rposition(|op| matches!(op, Operation::ReplaceGraph { .. }));
            if ops.is_empty() {
                refuse_lost_log(&path)?;
            }
            if last_import.is_none() && snapshot_path.exists() {
                restore(&mut graph, &snapshot_path)?;
            }
            for op in &ops[last_import.unwrap_or(0)..] {
                apply_op_to_memory(&mut graph, op, &path)?;
            }
            let keep = last_import.and_then(|at| match &ops[at] {
                Operation::ReplaceGraph { file } => Some(file.as_str()),
                _ => None,
            });
            // Best effort: garbage that can't be deleted now is retried at
            // the next open, and never stops this one.
            let _ = remove_stale_imports(&path, keep);
            if let Some(keep) = keep {
                base_bytes = std::fs::metadata(path.join(keep)).map_or(0, |meta| meta.len());
                // Replay starts at an import or checkpoint from now on.
                let _ = std::fs::remove_file(&snapshot_path);
            }
        }

        // Only a native disk database gets a checkpoint thread below.
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut zega = Zega {
            graph: Arc::new(Mutex::new(graph)),
            wal: Arc::new(wal),
            #[cfg(not(target_arch = "wasm32"))]
            path,
            in_memory: builder.in_memory,
            traversal_work_budget,
            query_time_limit: builder.query_time_limit,
            allow_private_imports: builder.allow_private_imports,
            #[cfg(not(target_arch = "wasm32"))]
            import_lock: Arc::new(Mutex::new(())),
            #[cfg(not(target_arch = "wasm32"))]
            base_bytes: Arc::new(std::sync::atomic::AtomicU64::new(base_bytes)),
            #[cfg(not(target_arch = "wasm32"))]
            checkpoint_counts: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            checkpointer: None,
        };
        #[cfg(not(target_arch = "wasm32"))]
        if !zega.in_memory && builder.snapshot_every > 0 {
            zega.checkpointer = Some(checkpoint::Checkpointer::spawn(
                zega.store(),
                builder.snapshot_every,
            )?);
        }
        Ok(zega)
    }

    /// Checkpoint a disk database: write the graph as a `.graph` file in
    /// `graphs/` and start the WAL over from it, so a restart reads that file
    /// and replays only what came after. The graph lock is held while the
    /// graph is encoded (one pass, into memory up to 256 MiB of `.graph`
    /// file), so reads and writes both wait that long: under 0.1 s for
    /// 100,000 nodes, about 1 s for 1,000,000 on an iMac. Writing the file,
    /// syncing it and rotating the WAL run without the lock. A no-op in
    /// memory. A disk
    /// database also does this on its own ([`ZegaBuilder::snapshot_every`]).
    pub fn snapshot(&self) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        self.checkpoint()?;
        Ok(())
    }

    /// [`Zega::snapshot`], saying what it wrote; `None` in memory.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn checkpoint(&self) -> Result<Option<Checkpoint>> {
        if self.in_memory {
            return Ok(None);
        }
        self.store().checkpoint().map(Some)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn store(&self) -> checkpoint::Store {
        checkpoint::Store {
            graph: Arc::clone(&self.graph),
            wal: Arc::clone(&self.wal),
            path: self.path.clone(),
            import_lock: Arc::clone(&self.import_lock),
            base_bytes: Arc::clone(&self.base_bytes),
            counts: Arc::clone(&self.checkpoint_counts),
        }
    }

    /// How many checkpoints this database has taken and failed since it
    /// opened, its own and [`Zega::snapshot`]'s. A failed one only means the
    /// WAL keeps every write until the next succeeds; each failure is also
    /// printed to stderr.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn checkpoint_counts(&self) -> CheckpointCounts {
        self.checkpoint_counts.get()
    }

    /// Whether the graph holds no node and no relationship.
    pub fn is_empty(&self) -> Result<bool> {
        let graph = self.lock_graph()?;
        Ok(graph.all_nodes().is_empty() && graph.all_relationships().is_empty())
    }

    /// Stream the whole graph to `out` as a `.graph` file
    /// (docs/graph-format.md). Memory beyond the graph itself stays bounded:
    /// the name dictionary and a 64 KiB buffer. Writers wait while it runs.
    pub fn export(&self, out: &mut impl std::io::Write) -> Result<graph_file::ExportSummary> {
        self.export_with(out, &graph_file::ExportOptions::default())
    }

    /// [`Zega::export`] with a schema and manifest metadata to carry along.
    pub fn export_with(
        &self,
        out: &mut impl std::io::Write,
        options: &graph_file::ExportOptions,
    ) -> Result<graph_file::ExportSummary> {
        let graph = self.lock_graph()?;
        Ok(graph_file::write(&graph, options, CREATED_BY, out)?)
    }

    /// Replace the whole graph with the `.graph` file read from `input`.
    ///
    /// All or nothing: the file is decoded and checked to its last byte into
    /// a new graph first, and only then installed, so on any error the graph
    /// is exactly what it was. A disk database keeps a copy of the file in
    /// `graphs/` and commits the import with one WAL entry naming it, so a
    /// crash leaves either the old graph or the new one, never a mix.
    pub fn import(&self, input: impl std::io::Read) -> Result<graph_file::ImportSummary> {
        #[cfg(not(target_arch = "wasm32"))]
        if !self.in_memory {
            return self.import_durably(input);
        }
        let (graph, summary) = graph_file::read(input)?;
        self.install(graph, None)?;
        Ok(summary)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn import_durably(&self, input: impl std::io::Read) -> Result<graph_file::ImportSummary> {
        use sha2::Digest;
        use std::io::Read;

        /// Copies every byte the decoder reads into the staging file.
        struct Tee<R, W> {
            input: R,
            copy: W,
            hash: sha2::Sha256,
        }
        impl<R: Read, W: std::io::Write> Read for Tee<R, W> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.input.read(buf)?;
                self.copy.write_all(&buf[..n])?;
                self.hash.update(&buf[..n]);
                Ok(n)
            }
        }

        let (file, staging) = self.create_staging("incoming")?;
        let mut tee = Tee {
            input,
            copy: std::io::BufWriter::new(file),
            hash: sha2::Sha256::new(),
        };
        let decoded = graph_file::read(&mut tee);
        // Every handle on the staging file is closed before it is renamed or
        // deleted (Windows refuses both on an open file), and a failed
        // cleanup never replaces the error that made it necessary.
        let Tee { copy, hash, .. } = tee;
        let result = decoded.map_err(ZegaError::from).and_then(|(graph, summary)| {
            let file = copy.into_inner().map_err(|error| error.into_error())?;
            self.commit_import(file, &staging, &hash.finalize(), graph)?;
            Ok(summary)
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&staging);
        }
        result
    }

    /// Import a `.graph` file already written to a staging file of this
    /// database ([`Zega::create_staging`]), without copying it: it is read
    /// once, then renamed to become the database's copy. The staging file is
    /// gone afterwards, whatever the outcome. A server spools an upload this
    /// way so a slow client never holds the database.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn import_staged(&self, staging: &std::path::Path) -> Result<graph_file::ImportSummary> {
        use sha2::Digest;
        use std::io::Read;

        struct Hashing<R> {
            input: R,
            hash: sha2::Sha256,
        }
        impl<R: Read> Read for Hashing<R> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.input.read(buf)?;
                self.hash.update(&buf[..n]);
                Ok(n)
            }
        }

        let result = (|| {
            let file = std::fs::OpenOptions::new().read(true).write(true).open(staging)?;
            if self.in_memory {
                return self.import(file);
            }
            let (graph, summary, digest) = {
                let mut hashing = Hashing {
                    input: file.try_clone()?,
                    hash: sha2::Sha256::new(),
                };
                let (graph, summary) = graph_file::read(&mut hashing)?;
                (graph, summary, hashing.hash.finalize())
                // The reading handle closes here, before the rename.
            };
            self.commit_import(file, staging, &digest, graph)?;
            Ok(summary)
        })();
        // Every handle is closed by now; the file is gone if it was
        // committed, and a failed delete never hides `result`.
        let _ = std::fs::remove_file(staging);
        result
    }

    /// A new, empty staging file in this database's `graphs/` directory, and
    /// its path: `None` for an in-memory database. Staging files a crash
    /// leaves behind are deleted the next time the database opens.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn staging_file(&self, purpose: &str) -> Result<Option<(std::fs::File, std::path::PathBuf)>> {
        if self.in_memory {
            return Ok(None);
        }
        Ok(Some(self.create_staging(purpose)?))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn create_staging(&self, purpose: &str) -> Result<(std::fs::File, std::path::PathBuf)> {
        create_staging(&self.path, purpose)
    }

    /// Make a fully checked staging file the database's copy of an import,
    /// commit it with one WAL entry, then install `graph`.
    #[cfg(not(target_arch = "wasm32"))]
    fn commit_import(
        &self,
        file: std::fs::File,
        staging: &std::path::Path,
        sha256: &[u8],
        graph: Graph,
    ) -> Result<()> {
        let name = format!("{IMPORTS_DIR}/{}.graph", graph_file::hex(sha256));
        let _one_import_at_a_time = self
            .import_lock
            .lock()
            .map_err(|_| ZegaError::Execution("import lock poisoned".to_string()))?;
        let bytes = file.metadata()?.len();
        crate::wal::persist_replacement(file, staging, &self.path.join(&name))?;
        self.wal.mark_imports()?;
        self.install(graph, Some(Operation::ReplaceGraph { file: name.clone() }))?;
        // The WAL starts from this file now: the next checkpoint waits for
        // the log to outgrow it.
        self.base_bytes.store(bytes, std::sync::atomic::Ordering::Relaxed);
        // Replay starts at this import, so no earlier imported file is read
        // again. Staging files may belong to requests in flight: kept. The
        // import is committed: a file that can't be deleted now (held open
        // by another process on Windows) is only garbage, removed at the
        // next open, and must not turn a committed import into an error.
        let _ = remove_imported_except(&self.path, &name);
        Ok(())
    }

    /// Swap in an imported graph once `entry` (if any) is durable in the WAL.
    fn install(&self, mut replacement: Graph, entry: Option<crate::wal::Operation>) -> Result<()> {
        let mut graph = self.lock_graph()?;
        if let Some(entry) = entry {
            self.wal.append(&entry)?;
        }
        replacement.inherit_statistics(&graph);
        *graph = replacement;
        Ok(())
    }

    /// Replace the graph with an empty one, durably, as one WAL entry (an
    /// import of an empty graph). What an earlier import carried (schema
    /// text, declarations, metadata) goes with it; the id counters stay, so
    /// no id is ever given out twice.
    pub fn clear(&self) -> Result<()> {
        let mut empty = Graph::new();
        empty.reset_next_ids(self.lock_graph()?.next_ids());
        let mut bytes = Vec::new();
        graph_file::write(&empty, &graph_file::ExportOptions::default(), CREATED_BY, &mut bytes)?;
        self.import(&bytes[..])?;
        Ok(())
    }

    /// This database's directory; `None` in memory.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn data_dir(&self) -> Option<&std::path::Path> {
        (!self.in_memory).then_some(self.path.as_path())
    }

    fn lock_graph(&self) -> Result<std::sync::MutexGuard<'_, Graph>> {
        self.graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))
    }

    /// Serialize the full graph state to bytes. Platform-independent —
    /// this is how the wasm build persists an in-memory database.
    pub fn snapshot_bytes(&self) -> Result<Vec<u8>> {
        let graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        Ok(crate::wal::encode_snapshot(&graph)?)
    }

    /// Restore the full graph state from [`snapshot_bytes`] output,
    /// replacing current state.
    pub fn restore_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut graph = self
            .graph
            .lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
        crate::wal::restore_bytes(&mut graph, bytes)?;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Zega {
    /// Stop the checkpoint thread (after the checkpoint it may be taking)
    /// before the WAL closes, so a reopen right after finds the files settled.
    fn drop(&mut self) {
        self.checkpointer.take();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_op_to_memory(graph: &mut Graph, op: &Operation, data: &std::path::Path) -> Result<()> {
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
        Operation::Statement { ops } => {
            for op in ops {
                apply_op_to_memory(graph, op, data)?;
            }
        }
        Operation::ReplaceGraph { file } => {
            let replacement = read_imported_graph(data, file)?;
            *graph = replacement;
        }
    }
    Ok(())
}

/// The graph an [`Operation::ReplaceGraph`] entry names, read back from the
/// data directory. It was fully checked before the entry was written, so a
/// failure here means the data directory lost or damaged it.
#[cfg(not(target_arch = "wasm32"))]
fn read_imported_graph(data: &std::path::Path, file: &str) -> Result<Graph> {
    let valid_name = file
        .strip_prefix(IMPORTS_DIR)
        .and_then(|name| name.strip_prefix('/'))
        .and_then(|name| name.strip_suffix(".graph"))
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
    if !valid_name {
        return Err(ZegaError::Execution(format!(
            "the WAL replaces the graph with {file:?}, which is not an imported .graph file name"
        )));
    }
    let path = data.join(file);
    let opened = std::fs::File::open(&path).map_err(|error| {
        ZegaError::Execution(format!(
            "the WAL replaces the graph with {}, which cannot be opened: {error}",
            path.display()
        ))
    })?;
    let (graph, _) = crate::graph_file::read(opened)?;
    Ok(graph)
}

/// A new, empty staging file in `data`'s `graphs/` directory, and its path.
/// Open deletes any a crash leaves behind.
#[cfg(not(target_arch = "wasm32"))]
fn create_staging(data: &std::path::Path, purpose: &str) -> Result<(std::fs::File, std::path::PathBuf)> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = data.join(IMPORTS_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        // The new directory entry is durable before anything in it is.
        sync_dir(data)?;
    }
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!(".{purpose}-{}-{sequence}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    // Read too: a checkpoint reads its file back for its digests.
    options.read(true).write(true).create_new(true);
    // Staging files become the database's copies of its imports: owner
    // only, like the WAL's contents deserve.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    Ok((options.open(&path)?, path))
}

/// Make a new entry in `dir` durable. Windows can't open a directory as a
/// file (`File::open` fails with "Access is denied"); NTFS journals the
/// directory change itself, which is what `persist_replacement` relies on
/// there too.
#[cfg(not(target_arch = "wasm32"))]
fn sync_dir(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(windows)]
    let _ = dir;
    Ok(())
}

/// Delete what no replay will read: imported files other than `keep` (the
/// last import, which replaces all earlier ones) and staging files left by a
/// crash. Runs at open, with the data directory to ourselves.
#[cfg(not(target_arch = "wasm32"))]
fn remove_stale_imports(data: &std::path::Path, keep: Option<&str>) -> Result<()> {
    remove_in_imports(data, |name| {
        let staging = name.starts_with('.') && name.ends_with(".tmp");
        // Only a log that names the graph it starts from says which files
        // are superseded. Without one, a `.graph` file is either the leftover
        // of an import or checkpoint that never committed (the log holds
        // every write, and the next checkpoint deletes it) or the only copy
        // of the graph ([`refuse_lost_log`]): never deleted here.
        let superseded = keep.and_then(file_name).is_some_and(|keep| name != keep);
        staging || (name.ends_with(".graph") && superseded)
    })
}

/// A log with no entries, next to `.graph` files in `graphs/`: the log that
/// named one of them is gone (deleted, or truncated to nothing), and the
/// graph is in that file. Opening would start an empty database; refuse, and
/// say how to get the graph back.
#[cfg(not(target_arch = "wasm32"))]
fn refuse_lost_log(data: &std::path::Path) -> Result<()> {
    let mut files: Vec<String> = match std::fs::read_dir(data.join(IMPORTS_DIR)) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.') && name.ends_with(".graph"))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if files.is_empty() {
        return Ok(());
    }
    files.sort();
    let files: Vec<String> = files.iter().map(|name| format!("{IMPORTS_DIR}/{name}")).collect();
    Err(ZegaError::Execution(format!(
        "{} holds no writes, but {} holds {}: the database's graph is in {} and the log \
         that named it is missing or empty. Refusing to open this as an empty database. \
         To recover, move {} out of {}, then run `zega import <file> --data {}` with it. \
         A file left by an import that never finished can be deleted instead",
        data.join("wal.bin").display(),
        data.join(IMPORTS_DIR).display(),
        files.join(", "),
        if files.len() == 1 { "that file" } else { "one of these files" },
        if files.len() == 1 { "it" } else { "them" },
        data.join(IMPORTS_DIR).display(),
        data.display(),
    )))
}

/// Delete every imported file but `keep`.
#[cfg(not(target_arch = "wasm32"))]
fn remove_imported_except(data: &std::path::Path, keep: &str) -> Result<()> {
    remove_in_imports(data, |name| {
        !name.starts_with('.') && name.ends_with(".graph") && Some(name) != file_name(keep)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn file_name(file: &str) -> Option<&str> {
    file.strip_prefix(IMPORTS_DIR)?.strip_prefix('/')
}

#[cfg(not(target_arch = "wasm32"))]
fn remove_in_imports(data: &std::path::Path, stale: impl Fn(&str) -> bool) -> Result<()> {
    let entries = match std::fs::read_dir(data.join(IMPORTS_DIR)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(&stale) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Where a disk database keeps the `.graph` files it has imported, named by
/// the SHA-256 of their bytes. The WAL entry of each import names its file.
#[cfg(not(target_arch = "wasm32"))]
const IMPORTS_DIR: &str = "graphs";

/// A snapshot from before checkpoints (zega#52): the graph with no WAL
/// position, the whole WAL replayed over it. Still read when the WAL holds no
/// import or checkpoint; deleted once it does.
#[cfg(not(target_arch = "wasm32"))]
const SNAPSHOT_FILE: &str = "snapshot.bin";
