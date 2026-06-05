use bincode::{deserialize_from, serialize_into};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zega_graph::{Graph, Node, NodeId, RelId, Relationship};
use zega_kv::KvStore;
use zega_parser::Value;

#[derive(Error, Debug)]
pub enum WalError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    InsertNode {
        id: NodeId,
        labels: Vec<String>,
        props: HashMap<String, Value>,
    },
    UpdateNode {
        id: NodeId,
        props: HashMap<String, Value>,
    },
    DeleteNode {
        id: NodeId,
    },
    InsertRel {
        id: RelId,
        kind: String,
        from: NodeId,
        to: NodeId,
        props: HashMap<String, Value>,
    },
    DeleteRel {
        id: RelId,
    },
    KvSet {
        key: String,
        value: Value,
        ttl: Option<u64>,
    },
    KvDel {
        key: String,
    },
}

pub struct Wal {
    path: PathBuf,
    file: Option<File>,
    flush_every: bool,
}

impl Wal {
    pub fn new(path: &Path, flush_every: bool) -> Result<Self, WalError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Wal {
            path: path.to_path_buf(),
            file: Some(file),
            flush_every,
        })
    }

    pub fn append(&mut self, op: &Operation) -> Result<(), WalError> {
        if let Some(ref mut file) = self.file {
            let bytes = bincode::serialize(op)?;
            let len = bytes.len() as u64;
            file.write_all(&len.to_le_bytes())?;
            file.write_all(&bytes)?;
            if self.flush_every {
                file.flush()?;
                file.sync_all()?;
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), WalError> {
        if let Some(ref mut file) = self.file {
            file.flush()?;
            file.sync_all()?;
        }
        Ok(())
    }

    pub fn iter(&self) -> Result<Vec<Operation>, WalError> {
        let file = File::open(&self.path)?;
        let mut reader = BufReader::new(file);
        let mut ops = Vec::new();
        loop {
            let mut len_bytes = [0u8; 8];
            if reader.read_exact(&mut len_bytes).is_err() {
                break;
            }
            let len = u64::from_le_bytes(len_bytes) as usize;
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            let op: Operation = bincode::deserialize(&buf)?;
            ops.push(op);
        }
        Ok(ops)
    }
}

pub fn snapshot(graph: &Graph, kv: &KvStore, path: &Path) -> Result<(), WalError> {
    let snapshot = Snapshot {
        nodes: graph.all_nodes().clone(),
        relationships: graph.all_relationships().clone(),
        kv_data: kv.snapshot(),
    };
    let file = File::create(path)?;
    serialize_into(file, &snapshot)?;
    Ok(())
}

pub fn restore(graph: &mut Graph, kv: &KvStore, path: &Path) -> Result<bool, WalError> {
    if !path.exists() {
        return Ok(false);
    }
    let file = File::open(path)?;
    let snapshot: Snapshot = deserialize_from(file)?;
    graph.set_state(snapshot.nodes, snapshot.relationships);
    kv.restore(snapshot.kv_data);
    Ok(true)
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    nodes: HashMap<NodeId, Node>,
    relationships: HashMap<RelId, Relationship>,
    kv_data: HashMap<String, zega_kv::KvEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn test_wal_roundtrip() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let mut wal = Wal::new(&wal_path, true).unwrap();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String("Alice".to_string()));
        wal.append(&Operation::InsertNode {
            id: 1,
            labels: vec!["Person".to_string()],
            props,
        }).unwrap();
        wal.append(&Operation::KvSet {
            key: "foo".to_string(),
            value: Value::String("bar".to_string()),
            ttl: None,
        }).unwrap();
        drop(wal);

        let wal2 = Wal::new(&wal_path, false).unwrap();
        let ops = wal2.iter().unwrap();
        assert_eq!(ops.len(), 2);
        match &ops[0] {
            Operation::InsertNode { id, labels, .. } => {
                assert_eq!(*id, 1);
                assert_eq!(labels, &vec!["Person".to_string()]);
            }
            _ => panic!("expected InsertNode"),
        }
        match &ops[1] {
            Operation::KvSet { key, value, .. } => {
                assert_eq!(key, "foo");
                assert_eq!(value, &Value::String("bar".to_string()));
            }
            _ => panic!("expected KvSet"),
        }
    }

    #[test]
    fn test_snapshot_restore() {
        let dir = tempdir().unwrap();
        let snap_path = dir.path().join("snap.bin");
        let mut graph = Graph::new();
        let kv = KvStore::new();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String("Alice".to_string()));
        graph.create_node(vec!["Person".to_string()], props);
        kv.set("foo".to_string(), Value::String("bar".to_string()), None);

        snapshot(&graph, &kv, &snap_path).unwrap();

        let mut graph2 = Graph::new();
        let kv2 = KvStore::new();
        restore(&mut graph2, &kv2, &snap_path).unwrap();

        assert_eq!(graph2.all_nodes().len(), 1);
        assert_eq!(kv2.get("foo"), Some(Value::String("bar".to_string())));
    }
}
