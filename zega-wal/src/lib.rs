#[cfg(not(target_arch = "wasm32"))]
use bincode::{deserialize_from, serialize_into};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use thiserror::Error;
use zega_graph::{Graph, NodeId, RelId};
#[cfg(not(target_arch = "wasm32"))]
use zega_graph::{Node, Relationship};
use zega_kv::KvStore;
use zega_parser::Value;

const WAL_MAGIC: &[u8; 4] = b"ZWAL";
const WAL_VERSION: u16 = 2;
const WAL_FILE_HEADER: &[u8; 6] = b"ZWAL\x02\x00";
const WAL_FILE_HEADER_LEN: u64 = WAL_FILE_HEADER.len() as u64;
const ENTRY_HEADER_LEN: u64 = 12;
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_GROUP_COMMIT_INTERVAL: Duration = Duration::from_millis(5);
const DEFAULT_GROUP_COMMIT_BATCH_SIZE: usize = 64;

#[derive(Error, Debug)]
pub enum WalError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("WAL corruption at byte {offset}: {reason}")]
    Corruption { offset: u64, reason: String },
    #[error("WAL durability error: {0}")]
    Durability(String),
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

#[cfg(not(target_arch = "wasm32"))]
struct WalState {
    file: Option<File>,
    next_sequence: u64,
    durable_sequence: u64,
    pending_entries: usize,
    durability_error: Option<String>,
    shutdown: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct GroupCommit {
    state: Mutex<WalState>,
    wake: Condvar,
    interval: Duration,
    batch_size: usize,
    flush_every: bool,
}

pub struct Wal {
    #[cfg(not(target_arch = "wasm32"))]
    path: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    group: Arc<GroupCommit>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<JoinHandle<()>>,
}

impl Wal {
    pub fn in_memory() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Wal {}
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Wal {
                path: PathBuf::new(),
                group: Arc::new(GroupCommit {
                    state: Mutex::new(WalState {
                        file: None,
                        next_sequence: 0,
                        durable_sequence: 0,
                        pending_entries: 0,
                        durability_error: None,
                        shutdown: false,
                    }),
                    wake: Condvar::new(),
                    interval: DEFAULT_GROUP_COMMIT_INTERVAL,
                    batch_size: DEFAULT_GROUP_COMMIT_BATCH_SIZE,
                    flush_every: false,
                }),
                worker: None,
            }
        }
    }

    pub fn new(path: &Path, flush_every: bool) -> Result<Self, WalError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, flush_every);
            Ok(Wal {})
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::with_group_commit(
                path,
                flush_every,
                DEFAULT_GROUP_COMMIT_INTERVAL,
                DEFAULT_GROUP_COMMIT_BATCH_SIZE,
            )
        }
    }

    pub fn with_group_commit(
        path: &Path,
        flush_every: bool,
        interval: std::time::Duration,
        batch_size: usize,
    ) -> Result<Self, WalError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, flush_every, interval, batch_size);
            Ok(Wal {})
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            prepare_wal(path)?;
            let file = OpenOptions::new().read(true).append(true).open(path)?;
            let group = Arc::new(GroupCommit {
                state: Mutex::new(WalState {
                    file: Some(file),
                    next_sequence: 0,
                    durable_sequence: 0,
                    pending_entries: 0,
                    durability_error: None,
                    shutdown: false,
                }),
                wake: Condvar::new(),
                interval,
                batch_size: batch_size.max(1),
                flush_every,
            });
            let worker = if flush_every {
                None
            } else {
                let group = Arc::clone(&group);
                Some(thread::spawn(move || group_commit_worker(group)))
            };
            Ok(Wal {
                path: path.to_path_buf(),
                group,
                worker,
            })
        }
    }

    pub fn append(&self, op: &Operation) -> Result<(), WalError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = op;
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let bytes = bincode::serialize(op)?;
            let len = u64::try_from(bytes.len())
                .map_err(|_| WalError::Durability("WAL entry exceeds u64 length".to_string()))?;
            let crc = crc32fast::hash(&bytes);
            let mut state = self
                .group
                .state
                .lock()
                .map_err(|_| WalError::Durability("group commit lock poisoned".to_string()))?;
            if let Some(error) = &state.durability_error {
                return Err(WalError::Durability(error.clone()));
            }
            if state.file.is_none() {
                return Ok(());
            }
            let append_result = {
                let file = state
                    .file
                    .as_mut()
                    .ok_or_else(|| WalError::Durability("WAL file is not available".to_string()))?;
                append_entry(file, len, crc, &bytes)
            };
            if let Err(error) = append_result {
                if matches!(error, WalError::Durability(_)) {
                    state.durability_error = Some(error.to_string());
                }
                return Err(error);
            }
            state.next_sequence += 1;
            let sequence = state.next_sequence;
            state.pending_entries += 1;

            if self.group.flush_every {
                sync_pending(&mut state)?;
                self.group.wake.notify_all();
                return Ok(());
            }
            if state.pending_entries >= self.group.batch_size {
                self.group.wake.notify_one();
            }
            while state.durable_sequence < sequence {
                state =
                    self.group.wake.wait(state).map_err(|_| {
                        WalError::Durability("group commit lock poisoned".to_string())
                    })?;
                if let Some(error) = &state.durability_error {
                    return Err(WalError::Durability(error.clone()));
                }
            }
            Ok(())
        }
    }

    pub fn flush(&self) -> Result<(), WalError> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut state = self
                .group
                .state
                .lock()
                .map_err(|_| WalError::Durability("group commit lock poisoned".to_string()))?;
            sync_pending(&mut state)?;
            self.group.wake.notify_all();
            Ok(())
        }
    }

    pub fn iter(&self) -> Result<Vec<Operation>, WalError> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(Vec::new())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
            let file_len = file.metadata()?.len();
            let mut reader = BufReader::new(file.try_clone()?);
            let mut ops = Vec::new();
            let mut header = [0u8; WAL_FILE_HEADER.len()];
            reader.read_exact(&mut header)?;
            if &header != WAL_FILE_HEADER {
                return Err(WalError::Corruption {
                    offset: 0,
                    reason: "invalid WAL header".to_string(),
                });
            }
            let mut valid_end = WAL_FILE_HEADER_LEN;

            while valid_end < file_len {
                let entry_start = valid_end;
                let remaining = file_len - entry_start;
                if remaining < ENTRY_HEADER_LEN {
                    truncate_tail(&file, valid_end)?;
                    break;
                }

                let mut len_bytes = [0u8; 8];
                reader.read_exact(&mut len_bytes)?;
                let len = u64::from_le_bytes(len_bytes);
                let mut crc_bytes = [0u8; 4];
                reader.read_exact(&mut crc_bytes)?;
                let expected_crc = u32::from_le_bytes(crc_bytes);
                let entry_end = entry_start
                    .checked_add(ENTRY_HEADER_LEN)
                    .and_then(|offset| offset.checked_add(len))
                    .ok_or_else(|| WalError::Corruption {
                        offset: entry_start,
                        reason: "entry length overflow".to_string(),
                    })?;
                if entry_end > file_len {
                    truncate_tail(&file, valid_end)?;
                    break;
                }
                let len = usize::try_from(len).map_err(|_| WalError::Corruption {
                    offset: entry_start,
                    reason: "entry is too large for this platform".to_string(),
                })?;
                let mut payload = vec![0u8; len];
                reader.read_exact(&mut payload)?;
                let actual_crc = crc32fast::hash(&payload);
                if actual_crc != expected_crc {
                    if entry_end == file_len {
                        truncate_tail(&file, valid_end)?;
                        break;
                    }
                    return Err(WalError::Corruption {
                        offset: entry_start,
                        reason: format!(
                            "checksum mismatch (expected {expected_crc:#010x}, got {actual_crc:#010x})"
                        ),
                    });
                }
                let op = bincode::deserialize(&payload).map_err(|error| WalError::Corruption {
                    offset: entry_start,
                    reason: format!("invalid operation payload: {error}"),
                })?;
                ops.push(op);
                valid_end = entry_end;
            }
            file.seek(SeekFrom::End(0))?;
            Ok(ops)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare_wal(path: &Path) -> Result<(), WalError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let file_len = file.metadata()?.len();
    if file_len == 0 {
        file.write_all(WAL_FILE_HEADER)?;
        file.sync_all()?;
        return Ok(());
    }

    let mut magic = [0u8; WAL_MAGIC.len()];
    let magic_len = file.read(&mut magic)?;
    if magic_len == WAL_MAGIC.len() && &magic == WAL_MAGIC {
        let mut version = [0u8; 2];
        file.read_exact(&mut version)
            .map_err(|_| WalError::Corruption {
                offset: 0,
                reason: "truncated WAL header".to_string(),
            })?;
        let version = u16::from_le_bytes(version);
        if version != WAL_VERSION {
            return Err(WalError::Corruption {
                offset: 0,
                reason: format!("unsupported WAL version {version}"),
            });
        }
        return Ok(());
    }

    migrate_legacy_wal(path, file_len)
}

#[cfg(not(target_arch = "wasm32"))]
fn migrate_legacy_wal(path: &Path, file_len: u64) -> Result<(), WalError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut entries = Vec::new();
    let mut offset = 0u64;
    while offset < file_len {
        let remaining = file_len - offset;
        if remaining < 8 {
            break;
        }
        let mut len_bytes = [0u8; 8];
        reader.read_exact(&mut len_bytes)?;
        let len = u64::from_le_bytes(len_bytes);
        let entry_end = offset
            .checked_add(8)
            .and_then(|start| start.checked_add(len))
            .ok_or_else(|| WalError::Corruption {
                offset,
                reason: "legacy entry length overflow".to_string(),
            })?;
        if entry_end > file_len {
            break;
        }
        let len = usize::try_from(len).map_err(|_| WalError::Corruption {
            offset,
            reason: "legacy entry is too large for this platform".to_string(),
        })?;
        let mut payload = vec![0u8; len];
        reader.read_exact(&mut payload)?;
        bincode::deserialize::<Operation>(&payload).map_err(|error| WalError::Corruption {
            offset,
            reason: format!("invalid legacy operation payload: {error}"),
        })?;
        entries.push(payload);
        offset = entry_end;
    }

    let tmp_path = path.with_extension("wal.migrate.tmp");
    let mut migrated = File::create(&tmp_path)?;
    migrated.write_all(WAL_FILE_HEADER)?;
    for payload in entries {
        let len = u64::try_from(payload.len())
            .map_err(|_| WalError::Durability("WAL entry exceeds u64 length".to_string()))?;
        let crc = crc32fast::hash(&payload);
        migrated.write_all(&len.to_le_bytes())?;
        migrated.write_all(&crc.to_le_bytes())?;
        migrated.write_all(&payload)?;
    }
    migrated.sync_all()?;
    drop(migrated);
    std::fs::rename(&tmp_path, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
trait AppendTarget: Write {
    fn len(&self) -> io::Result<u64>;
    fn truncate(&mut self, len: u64) -> io::Result<()>;
}

#[cfg(not(target_arch = "wasm32"))]
impl AppendTarget for File {
    fn len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn append_entry<T: AppendTarget>(
    target: &mut T,
    len: u64,
    crc: u32,
    payload: &[u8],
) -> Result<(), WalError> {
    let offset = target.len()?;
    let result = target
        .write_all(&len.to_le_bytes())
        .and_then(|()| target.write_all(&crc.to_le_bytes()))
        .and_then(|()| target.write_all(payload));
    if let Err(write_error) = result {
        target.truncate(offset).map_err(|truncate_error| {
            WalError::Durability(format!(
                "WAL append failed ({write_error}); rollback failed ({truncate_error})"
            ))
        })?;
        return Err(WalError::Io(write_error));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Wal {
    fn drop(&mut self) {
        if let Ok(mut state) = self.group.state.lock() {
            state.shutdown = true;
            self.group.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_pending(state: &mut WalState) -> Result<(), WalError> {
    if state.pending_entries == 0 {
        return Ok(());
    }
    if let Some(file) = state.file.as_mut() {
        file.flush()?;
        file.sync_all()?;
    }
    state.durable_sequence = state.next_sequence;
    state.pending_entries = 0;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn truncate_tail(file: &File, valid_end: u64) -> Result<(), WalError> {
    file.set_len(valid_end)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn group_commit_worker(group: Arc<GroupCommit>) {
    loop {
        let mut state = match group.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        while !state.shutdown && state.pending_entries < group.batch_size {
            let result = group.wake.wait_timeout(state, group.interval);
            match result {
                Ok((next, timeout)) => {
                    state = next;
                    if timeout.timed_out() && state.pending_entries > 0 {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        if state.shutdown {
            let _ = sync_pending(&mut state);
            group.wake.notify_all();
            return;
        }
        if let Err(error) = sync_pending(&mut state) {
            state.durability_error = Some(error.to_string());
            group.wake.notify_all();
            return;
        }
        group.wake.notify_all();
    }
}

pub fn snapshot(graph: &Graph, kv: &KvStore, path: &Path) -> Result<(), WalError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (graph, kv, path);
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let snapshot = Snapshot {
            nodes: graph.all_nodes().clone(),
            relationships: graph.all_relationships().clone(),
            kv_data: kv.snapshot(),
        };
        let tmp_path = path.with_extension("bin.tmp");
        let mut file = File::create(&tmp_path)?;
        serialize_into(&mut file, &snapshot)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp_path, path)?;
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

pub fn restore(graph: &mut Graph, kv: &KvStore, path: &Path) -> Result<bool, WalError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (graph, kv, path);
        Ok(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !path.exists() {
            return Ok(false);
        }
        let file = File::open(path)?;
        let snapshot: Snapshot = deserialize_from(file)?;
        graph.set_state(snapshot.nodes, snapshot.relationships);
        kv.restore(snapshot.kv_data);
        Ok(true)
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
    use std::io::{BufRead, BufReader as ProcessBufReader};
    use std::process::{Command, Stdio};
    use tempfile::tempdir;

    fn kv_set(key: &str) -> Operation {
        Operation::KvSet {
            key: key.to_string(),
            value: Value::String(key.to_string()),
            ttl: None,
        }
    }

    fn kv_keys(ops: &[Operation]) -> Vec<&str> {
        ops.iter()
            .map(|op| match op {
                Operation::KvSet { key, .. } => key.as_str(),
                _ => panic!("expected KvSet operation"),
            })
            .collect()
    }

    #[test]
    fn test_wal_roundtrip() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String("Alice".to_string()));
        wal.append(&Operation::InsertNode {
            id: 1,
            labels: vec!["Person".to_string()],
            props,
        })
        .unwrap();
        wal.append(&kv_set("foo")).unwrap();
        drop(wal);

        let wal2 = Wal::new(&wal_path, false).unwrap();
        assert_eq!(wal2.iter().unwrap().len(), 2);
    }

    #[test]
    fn legacy_wal_is_migrated_without_data_loss() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let expected = [kv_set("legacy-one"), kv_set("legacy-two")];
        let mut legacy = File::create(&wal_path).unwrap();
        for op in &expected {
            let payload = bincode::serialize(op).unwrap();
            legacy
                .write_all(&(payload.len() as u64).to_le_bytes())
                .unwrap();
            legacy.write_all(&payload).unwrap();
        }
        legacy.sync_all().unwrap();
        drop(legacy);

        let wal = Wal::new(&wal_path, true).unwrap();
        let recovered = wal.iter().unwrap();
        assert_eq!(kv_keys(&recovered), ["legacy-one", "legacy-two"]);
        assert!(std::fs::read(&wal_path)
            .unwrap()
            .starts_with(WAL_FILE_HEADER));
    }

    struct PartialWriteTarget {
        file: File,
        bytes_before_error: usize,
        failed: bool,
    }

    impl Write for PartialWriteTarget {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.failed {
                return Err(io::Error::other("injected partial write failure"));
            }
            let written = self.bytes_before_error.min(buf.len());
            let written = self.file.write(&buf[..written])?;
            self.bytes_before_error -= written;
            if self.bytes_before_error == 0 {
                self.failed = true;
            }
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl AppendTarget for PartialWriteTarget {
        fn len(&self) -> io::Result<u64> {
            Ok(self.file.metadata()?.len())
        }

        fn truncate(&mut self, len: u64) -> io::Result<()> {
            self.file.set_len(len)
        }
    }

    #[test]
    fn partial_append_is_truncated_before_later_append() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&kv_set("before")).unwrap();
        drop(wal);
        let valid_len = std::fs::metadata(&wal_path).unwrap().len();

        let payload = bincode::serialize(&kv_set("partial")).unwrap();
        let mut target = PartialWriteTarget {
            file: OpenOptions::new().append(true).open(&wal_path).unwrap(),
            bytes_before_error: 10,
            failed: false,
        };
        assert!(append_entry(
            &mut target,
            payload.len() as u64,
            crc32fast::hash(&payload),
            &payload
        )
        .is_err());
        assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), valid_len);
        drop(target);

        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&kv_set("after")).unwrap();
        assert_eq!(kv_keys(&wal.iter().unwrap()), ["before", "after"]);
    }

    #[test]
    fn torn_trailing_entry_is_truncated() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&kv_set("acked")).unwrap();
        drop(wal);
        let valid_len = std::fs::metadata(&wal_path).unwrap().len();
        let mut file = OpenOptions::new().append(true).open(&wal_path).unwrap();
        file.write_all(&100u64.to_le_bytes()).unwrap();
        file.write_all(&0u32.to_le_bytes()).unwrap();
        file.write_all(b"partial").unwrap();
        drop(file);

        let wal = Wal::new(&wal_path, false).unwrap();
        assert_eq!(wal.iter().unwrap().len(), 1);
        assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), valid_len);
    }

    #[test]
    fn corrupt_trailing_checksum_is_truncated() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&kv_set("acked")).unwrap();
        wal.append(&kv_set("tail")).unwrap();
        drop(wal);
        let mut bytes = std::fs::read(&wal_path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&wal_path, bytes).unwrap();

        let wal = Wal::new(&wal_path, false).unwrap();
        assert_eq!(wal.iter().unwrap().len(), 1);
    }

    #[test]
    fn corrupt_middle_checksum_is_an_error() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&kv_set("first")).unwrap();
        wal.append(&kv_set("second")).unwrap();
        drop(wal);
        let mut bytes = std::fs::read(&wal_path).unwrap();
        bytes[(WAL_FILE_HEADER_LEN + ENTRY_HEADER_LEN) as usize] ^= 0xff;
        std::fs::write(&wal_path, bytes).unwrap();

        let wal = Wal::new(&wal_path, false).unwrap();
        assert!(matches!(wal.iter(), Err(WalError::Corruption { .. })));
    }

    #[test]
    fn group_commit_acknowledges_concurrent_writes() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal =
            Arc::new(Wal::with_group_commit(&wal_path, false, Duration::from_secs(1), 4).unwrap());
        let threads: Vec<_> = (0..4)
            .map(|index| {
                let wal = Arc::clone(&wal);
                thread::spawn(move || wal.append(&kv_set(&format!("key-{index}"))).unwrap())
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(wal.iter().unwrap().len(), 4);
    }

    #[test]
    fn crash_writer_helper() {
        let Some(path) = std::env::var_os("ZEGA_CRASH_WRITER_PATH") else {
            return;
        };
        let wal = Wal::new(Path::new(&path), false).unwrap();
        for index in 0..1_000 {
            wal.append(&kv_set(&format!("acked-{index}"))).unwrap();
            println!("ACK {index}");
            std::io::stdout().flush().unwrap();
        }
    }

    #[test]
    fn acknowledged_writes_survive_kill_9() {
        for iteration in 0..3 {
            let dir = tempdir().unwrap();
            let wal_path = dir.path().join("wal.bin");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::crash_writer_helper", "--nocapture"])
                .env("ZEGA_CRASH_WRITER_PATH", &wal_path)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let mut lines = ProcessBufReader::new(stdout).lines();
            let target = 5 + iteration * 4;
            let mut acknowledged = 0;
            while acknowledged < target {
                let line = lines.next().unwrap().unwrap();
                if line.starts_with("ACK ") {
                    acknowledged += 1;
                }
            }
            child.kill().unwrap();
            child.wait().unwrap();

            let wal = Wal::new(&wal_path, false).unwrap();
            let recovered = wal.iter().unwrap();
            assert!(
                recovered.len() >= acknowledged,
                "iteration {iteration}: recovered {} of {acknowledged} acknowledged writes",
                recovered.len()
            );
        }
    }

    #[test]
    fn test_snapshot_restore() {
        let dir = tempdir().unwrap();
        let snap_path = dir.path().join("snapshot.bin");
        let mut graph = Graph::new();
        let kv = KvStore::new();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String("Alice".to_string()));
        graph.create_node(vec!["Person".to_string()], props);
        kv.set("foo".to_string(), Value::String("bar".to_string()), None);

        snapshot(&graph, &kv, &snap_path).unwrap();
        assert!(!snap_path.with_extension("bin.tmp").exists());

        let mut graph2 = Graph::new();
        let kv2 = KvStore::new();
        restore(&mut graph2, &kv2, &snap_path).unwrap();
        assert_eq!(graph2.all_nodes().len(), 1);
        assert_eq!(kv2.get("foo"), Some(Value::String("bar".to_string())));
    }
}
