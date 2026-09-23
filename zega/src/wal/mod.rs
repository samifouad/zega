use bincode::{serialize_into, Options};
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
use crate::graph::{Graph, Node, NodeId, RelId, Relationship};
use crate::parser::Value;

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

    // Only this module's own durability tests call `new`/`flush` directly;
    // `Zega` always goes through `with_group_commit`. Kept public and
    // allowed here rather than deleted: it's real WAL API, not dead code.
    #[allow(dead_code)]
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

    // On wasm32 `Wal` is a stub with no file I/O (see `new` above), so this
    // constructor is only reachable on the native, file-backed path.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
            let file = open_wal_writer(path)?;
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

    #[allow(dead_code)]
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

    // Replays the on-disk log; nothing to replay for the wasm32 stub.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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

    file.seek(SeekFrom::Start(0))?;
    migrate_legacy_wal(path, file, file_len)
}

#[cfg(not(target_arch = "wasm32"))]
fn migrate_legacy_wal(path: &Path, file: File, file_len: u64) -> Result<(), WalError> {
    let mut reader = BufReader::new(file);
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
    // Release the original WAL before replacing it. The append writer is
    // opened by with_group_commit only after migration has completed.
    drop(reader);

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
    persist_replacement(migrated, &tmp_path, path)
}

// Both callers write a sibling temporary file and release all destination
// handles before entering here. Never remove the destination before replacing
// it: a failed rename must leave the last durable version available.
#[cfg(not(target_arch = "wasm32"))]
fn persist_replacement(file: File, tmp_path: &Path, path: &Path) -> Result<(), WalError> {
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        // Canonical parents produce absolute verbatim paths, including for
        // a new destination, so Unicode and paths beyond MAX_PATH still work.
        fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "replacement needs a file name")
            })?;
            let mut wide: Vec<u16> = parent
                .canonicalize()?
                .join(name)
                .as_os_str()
                .encode_wide()
                .collect();
            if wide.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path contains NUL",
                ));
            }
            wide.push(0);
            Ok(wide)
        }
        let from = wide_path(tmp_path)?;
        let to = wide_path(path)?;
        // Windows cannot use File::open(directory).sync_all(). Request a
        // write-through rename instead; COPY_ALLOWED is deliberately absent.
        // https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw
        // SAFETY: both pointers refer to live, NUL-terminated UTF-16 buffers.
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(tmp_path, path)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn open_wal_writer(path: &Path) -> io::Result<File> {
    // Windows append-only handles lack FILE_WRITE_DATA, which set_len needs
    // to roll back partial writes. append_entry seeks under the WAL mutex.
    OpenOptions::new().read(true).write(true).open(path)
}

#[cfg(not(target_arch = "wasm32"))]
trait AppendTarget: Write {
    fn seek_end(&mut self) -> io::Result<u64>;
    fn truncate(&mut self, len: u64) -> io::Result<()>;
}

#[cfg(not(target_arch = "wasm32"))]
impl AppendTarget for File {
    fn seek_end(&mut self) -> io::Result<u64> {
        self.seek(SeekFrom::End(0))
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
    // Seek for every append, including after rollback: set_len does not move
    // the cursor, and another handle may have truncated a torn tail.
    let offset = target.seek_end()?;
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

// `Zega::open`/`Zega::snapshot` only call this on the native, file-backed
// path (see the `cfg(not(target_arch = "wasm32"))` call sites in lib.rs);
// the wasm32 build persists through `encode_snapshot`/`restore_bytes` instead.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn snapshot(graph: &Graph, path: &Path) -> Result<(), WalError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (graph, path);
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes = encode_snapshot(graph)?;
        let tmp_path = path.with_extension("bin.tmp");
        let mut file = File::create(&tmp_path)?;
        file.write_all(&bytes)?;
        file.flush()?;
        persist_replacement(file, &tmp_path, path)
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn restore(graph: &mut Graph, path: &Path) -> Result<bool, WalError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (graph, path);
        Ok(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !path.exists() {
            return Ok(false);
        }
        let bytes = std::fs::read(path)?;
        restore_bytes(graph, &bytes)?;
        Ok(true)
    }
}

/// Serialize the full graph state to bytes (platform-independent; the
/// basis for the file-based snapshot and for wasm export/import).
pub fn encode_snapshot(graph: &Graph) -> Result<Vec<u8>, WalError> {
    let snapshot = Snapshot {
        nodes: graph.all_nodes().clone(),
        relationships: graph.all_relationships().clone(),
    };
    let mut bytes = Vec::new();
    serialize_into(&mut bytes, &snapshot)?;
    Ok(bytes)
}

/// Restore the full graph state from [`encode_snapshot`] bytes.
pub fn restore_bytes(graph: &mut Graph, bytes: &[u8]) -> Result<(), WalError> {
    let snapshot: Snapshot = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .with_limit(bytes.len() as u64)
        .deserialize(bytes)
        .map_err(|error| WalError::Corruption {
            offset: 0,
            reason: format!("invalid snapshot: {error}"),
        })?;
    graph.set_state(snapshot.nodes, snapshot.relationships);
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    nodes: HashMap<NodeId, Node>,
    relationships: HashMap<RelId, Relationship>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader as ProcessBufReader};
    use std::process::{Command, Stdio};
    use tempfile::tempdir;

    fn insert_node(label: &str) -> Operation {
        Operation::InsertNode {
            id: 1,
            labels: vec![label.to_string()],
            props: HashMap::new(),
        }
    }

    fn node_labels(ops: &[Operation]) -> Vec<&str> {
        ops.iter()
            .map(|op| match op {
                Operation::InsertNode { labels, .. } => labels[0].as_str(),
                _ => panic!("expected InsertNode operation"),
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
        wal.append(&insert_node("foo")).unwrap();
        drop(wal);

        let wal2 = Wal::new(&wal_path, false).unwrap();
        assert_eq!(wal2.iter().unwrap().len(), 2);
    }

    #[test]
    fn legacy_wal_is_migrated_without_data_loss() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let expected = [insert_node("legacy-one"), insert_node("legacy-two")];
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
        assert_eq!(node_labels(&recovered), ["legacy-one", "legacy-two"]);
        assert!(std::fs::read(&wal_path)
            .unwrap()
            .starts_with(WAL_FILE_HEADER));
    }

    #[test]
    fn replacement_supports_long_unicode_paths() {
        let dir = tempdir().unwrap();
        let mut path = dir.path().to_path_buf();
        for _ in 0..6 {
            path.push("storage-世界-🦀-abcdefghijklmnopqrstuvwxyz-0123456789");
        }
        std::fs::create_dir_all(&path).unwrap();
        let snap_path = path.join("snapshot-世界.bin");
        snapshot(&Graph::new(), &snap_path).unwrap();
        let mut graph = Graph::new();
        graph.create_node(vec!["saved".to_string()], HashMap::new());
        snapshot(&graph, &snap_path).unwrap();
        let mut restored = Graph::new();
        assert!(restore(&mut restored, &snap_path).unwrap());
        assert_eq!(restored.all_nodes().len(), 1);
        assert!(!snap_path.with_extension("bin.tmp").exists());

        let wal_path = path.join("wal-世界.bin");
        let payload = bincode::serialize(&insert_node("legacy")).unwrap();
        let mut legacy = File::create(&wal_path).unwrap();
        legacy
            .write_all(&(payload.len() as u64).to_le_bytes())
            .unwrap();
        legacy.write_all(&payload).unwrap();
        legacy.sync_all().unwrap();
        drop(legacy);
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&insert_node("new")).unwrap();
        drop(wal);
        let wal = Wal::new(&wal_path, true).unwrap();
        assert_eq!(node_labels(&wal.iter().unwrap()), ["legacy", "new"]);
        assert!(!wal_path.with_extension("wal.migrate.tmp").exists());
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
        fn seek_end(&mut self) -> io::Result<u64> {
            self.file.seek_end()
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
        wal.append(&insert_node("before")).unwrap();
        drop(wal);
        let valid_len = std::fs::metadata(&wal_path).unwrap().len();

        let payload = bincode::serialize(&insert_node("partial")).unwrap();
        let mut target = PartialWriteTarget {
            file: open_wal_writer(&wal_path).unwrap(),
            bytes_before_error: 10,
            failed: false,
        };
        let error = append_entry(
            &mut target,
            payload.len() as u64,
            crc32fast::hash(&payload),
            &payload,
        )
        .unwrap_err();
        assert!(matches!(error, WalError::Io(_)), "rollback failed: {error}");
        assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), valid_len);
        // Continue on the SAME handle: a successful rollback must also leave
        // later appends at EOF rather than at the old partial-write cursor.
        target.failed = false;
        target.bytes_before_error = usize::MAX;
        let payload = bincode::serialize(&insert_node("same-handle")).unwrap();
        append_entry(
            &mut target,
            payload.len() as u64,
            crc32fast::hash(&payload),
            &payload,
        )
        .unwrap();
        target.file.sync_all().unwrap();
        drop(target);

        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&insert_node("after")).unwrap();
        assert_eq!(
            node_labels(&wal.iter().unwrap()),
            ["before", "same-handle", "after"]
        );
    }

    #[test]
    fn torn_trailing_entry_is_truncated() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append(&insert_node("acked")).unwrap();
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
        wal.append(&insert_node("acked")).unwrap();
        wal.append(&insert_node("tail")).unwrap();
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
        wal.append(&insert_node("first")).unwrap();
        wal.append(&insert_node("second")).unwrap();
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
                thread::spawn(move || wal.append(&insert_node(&format!("key-{index}"))).unwrap())
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
            wal.append(&insert_node(&format!("acked-{index}"))).unwrap();
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
                .args(["--exact", "wal::tests::crash_writer_helper", "--nocapture"])
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
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::String("Alice".to_string()));
        graph.create_node(vec!["Person".to_string()], props);

        snapshot(&graph, &snap_path).unwrap();
        assert!(!snap_path.with_extension("bin.tmp").exists());

        let mut graph2 = Graph::new();
        restore(&mut graph2, &snap_path).unwrap();
        assert_eq!(graph2.all_nodes().len(), 1);
    }
}

#[cfg(test)]
mod exhaustive_tests;
