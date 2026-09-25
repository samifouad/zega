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
}

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
                apply_op_to_memory(&mut graph, &op);
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
fn apply_op_to_memory(graph: &mut Graph, op: &Operation) {
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
                apply_op_to_memory(graph, op);
            }
        }
    }
}
