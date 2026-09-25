pub mod location;
pub mod vector;
mod vector_view;
use std::path::PathBuf;
use std::sync::Mutex;
use thiserror::Error;
use crate::graph::Graph;
pub use crate::value::Value;
#[cfg(not(target_arch = "wasm32"))]
use crate::wal::{restore, snapshot};
#[cfg(not(target_arch = "wasm32"))]
use crate::wal::Operation;
use crate::wal::Wal;

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
    graph: Mutex<Graph>,
    wal: Wal,
    #[cfg(not(target_arch = "wasm32"))]
    path: PathBuf,
    in_memory: bool,
    traversal_work_budget: usize,
    query_time_limit: Option<std::time::Duration>,
    allow_private_imports: bool,
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
        let snapshot_path = path.join("snapshot.bin");
        let wal_path = path.join("wal.bin");

        // Restore from snapshot if exists
        #[cfg(not(target_arch = "wasm32"))]
        if !builder.in_memory && snapshot_path.exists() {
            restore(&mut graph, &snapshot_path)?;
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
        if !builder.in_memory && wal_path.exists() {
            let ops = wal.iter()?;
            for op in ops {
                apply_op_to_memory(&mut graph, &op, &path)?;
            }
        }

        Ok(Zega {
            graph: Mutex::new(graph),
            wal,
            #[cfg(not(target_arch = "wasm32"))]
            path,
            in_memory: builder.in_memory,
            traversal_work_budget,
            query_time_limit: builder.query_time_limit,
            allow_private_imports: builder.allow_private_imports,
        })
    }

    pub fn snapshot(&self) -> Result<()> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.in_memory {
                return Ok(());
            }
            let graph = self
                .graph
                .lock()
                .map_err(|_| ZegaError::Execution("lock poisoned".to_string()))?;
            let snapshot_path = self.path.join("snapshot.bin");
            snapshot(&graph, &snapshot_path)?;
            Ok(())
        }
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
        use sha2::{Digest, Sha256};
        use std::io::Read;

        /// Copies every byte the decoder reads into the staging file.
        struct Tee<R, W> {
            input: R,
            copy: W,
            hash: Sha256,
        }
        impl<R: Read, W: std::io::Write> Read for Tee<R, W> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.input.read(buf)?;
                self.copy.write_all(&buf[..n])?;
                self.hash.update(&buf[..n]);
                Ok(n)
            }
        }

        let dir = self.path.join(IMPORTS_DIR);
        std::fs::create_dir_all(&dir)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let staging = dir.join(format!(".incoming-{}-{nanos}.tmp", std::process::id()));
        let mut tee = Tee {
            input,
            copy: std::io::BufWriter::new(std::fs::File::create(&staging)?),
            hash: Sha256::new(),
        };
        let decoded = graph_file::read(&mut tee);
        let committed = decoded.and_then(|(graph, summary)| {
            let file = tee.copy.into_inner().map_err(|error| error.into_error())?;
            let name = format!("{IMPORTS_DIR}/{}.graph", graph_file::hex(&tee.hash.finalize()));
            crate::wal::persist_replacement(file, &staging, &self.path.join(&name))
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            Ok((graph, summary, name))
        });
        match committed {
            Ok((graph, summary, file)) => {
                self.install(graph, Some(Operation::ReplaceGraph { file }))?;
                Ok(summary)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&staging);
                Err(error.into())
            }
        }
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

/// Where a disk database keeps the `.graph` files it has imported, named by
/// the SHA-256 of their bytes. The WAL entry of each import names its file.
#[cfg(not(target_arch = "wasm32"))]
const IMPORTS_DIR: &str = "graphs";
