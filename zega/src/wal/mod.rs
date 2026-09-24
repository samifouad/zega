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
    /// Every write of one statement, in order, as a single entry: one CRC
    /// covers them all, so replay sees the whole statement or none of it.
    /// New variants go last so entries written before them still decode.
    Statement {
        ops: Vec<Operation>,
    },
}

#[cfg(not(target_arch = "wasm32"))]
struct WalState {
    file: Option<Box<dyn AppendTarget + Send>>,
    next_sequence: u64,
    durable_sequence: u64,
    pending_entries: usize,
    /// File offset where the oldest unsynced entry starts: a failed sync
    /// truncates back to here, so writes reported as failed leave the file.
    pending_start: u64,
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
                        pending_start: 0,
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
            Ok(Self::from_target(
                path,
                Box::new(file),
                flush_every,
                interval,
                batch_size,
            ))
        }
    }

    /// Builds a file-backed WAL around an already opened append target.
    /// `with_group_commit` passes the real WAL file; the durability tests
    /// pass a fault-injecting target through this same path.
    #[cfg(not(target_arch = "wasm32"))]
    fn from_target(
        path: &Path,
        target: Box<dyn AppendTarget + Send>,
        flush_every: bool,
        interval: Duration,
        batch_size: usize,
    ) -> Self {
        let group = Arc::new(GroupCommit {
            state: Mutex::new(WalState {
                file: Some(target),
                next_sequence: 0,
                durable_sequence: 0,
                pending_entries: 0,
                pending_start: 0,
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
        Wal {
            path: path.to_path_buf(),
            group,
            worker,
        }
    }

    /// Append one statement's writes as a unit: nothing for none, the bare
    /// operation for one (the same bytes a single write always had), and a
    /// [`Operation::Statement`] entry for more, so a failure part-way through
    /// cannot leave half a statement in the log.
    pub fn append_statement(&self, mut ops: Vec<Operation>) -> Result<(), WalError> {
        match ops.len() {
            0 => Ok(()),
            1 => self.append(&ops.remove(0)),
            _ => self.append(&Operation::Statement { ops }),
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
                append_entry(file.as_mut(), len, crc, &bytes)
            };
            let offset = match append_result {
                Ok(offset) => offset,
                Err(error) => {
                    // Poison: every later append returns this error until the
                    // store is reopened. Keep the bare message so the replayed
                    // error reads the same as the first one.
                    if let WalError::Durability(message) = &error {
                        state.durability_error = Some(message.clone());
                    }
                    return Err(error);
                }
            };
            if state.pending_entries == 0 {
                state.pending_start = offset;
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
                let op = decode_exact(&payload).map_err(|error| WalError::Corruption {
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

/// Decode exactly one `T` from `bytes`, the way every file this module reads
/// was written: bincode's fixint encoding (what `bincode::serialize` writes),
/// nothing left over, and no length prefix allowed to claim more than `bytes`
/// holds. WAL entries, legacy WAL entries and snapshots all decode here, so a
/// frame that is not exactly one value is corruption wherever it is read.
fn decode_exact<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, bincode::Error> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .with_limit(bytes.len() as u64)
        .deserialize(bytes)
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
        decode_exact::<Operation>(&payload).map_err(|error| WalError::Corruption {
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
    /// Makes every earlier write and truncate durable, including the file
    /// length. A truncate changes no directory entry, so no platform needs
    /// the parent directory synced for it (unlike `persist_replacement`).
    fn sync(&mut self) -> io::Result<()>;
}

#[cfg(not(target_arch = "wasm32"))]
impl AppendTarget for File {
    fn seek_end(&mut self) -> io::Result<u64> {
        self.seek(SeekFrom::End(0))
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }

    // sync_all, not sync_data: the rollback changes only the file length,
    // which is metadata. On Windows this is FlushFileBuffers, which needs
    // the write access open_wal_writer already requests.
    fn sync(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Writes one entry at the end of the file and returns the offset it starts
/// at; on a failed write, nothing of it is left in the file.
fn append_entry<T: AppendTarget + ?Sized>(
    target: &mut T,
    len: u64,
    crc: u32,
    payload: &[u8],
) -> Result<u64, WalError> {
    // Seek for every append, including after rollback: set_len does not move
    // the cursor, and another handle may have truncated a torn tail.
    let offset = target.seek_end()?;
    let result = target
        .write_all(&len.to_le_bytes())
        .and_then(|()| target.write_all(&crc.to_le_bytes()))
        .and_then(|()| target.write_all(payload));
    if let Err(write_error) = result {
        // The rollback is only a rollback once it is durable: an unsynced
        // truncate can leave the torn tail on disk after a crash. Either
        // failure is a Durability error, which poisons the WAL (see
        // `Wal::append`) until the store is reopened and replay repairs it.
        target.truncate(offset).map_err(|truncate_error| {
            WalError::Durability(format!(
                "WAL append failed ({write_error}); rollback to byte {offset} failed \
                 ({truncate_error}); the WAL may be inconsistent until the store is reopened"
            ))
        })?;
        target.sync().map_err(|sync_error| {
            WalError::Durability(format!(
                "WAL append failed ({write_error}); rollback to byte {offset} could not be \
                 synced ({sync_error}); the WAL may be inconsistent until the store is reopened"
            ))
        })?;
        return Err(WalError::Io(write_error));
    }
    Ok(offset)
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
        if let Err(sync_error) = file.flush().and_then(|()| file.sync()) {
            // Every caller waiting on these entries is told its write failed
            // and discards it, so the entries must leave the file too, or a
            // reopen would replay them. After a failed fsync the handle cannot
            // be trusted again (fsyncgate), so poison whatever happens.
            let start = state.pending_start;
            let message = match file.truncate(start).and_then(|()| file.sync()) {
                Ok(()) => format!(
                    "WAL sync failed ({sync_error}); the unsynced writes were removed \
                     and the WAL refuses writes until the store is reopened"
                ),
                Err(rollback_error) => format!(
                    "WAL sync failed ({sync_error}); removing the unsynced writes failed \
                     ({rollback_error}); the WAL may be inconsistent until the store is reopened"
                ),
            };
            state.pending_entries = 0;
            state.durability_error = Some(message.clone());
            return Err(WalError::Durability(message));
        }
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
            state.durability_error.get_or_insert_with(|| error.to_string());
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
    let snapshot: Snapshot = decode_exact(bytes).map_err(|error| WalError::Corruption {
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
pub(crate) mod tests {
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

        fn sync(&mut self) -> io::Result<()> {
            self.file.sync_all()
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

    /// What a `FaultyTarget` did to the real WAL file underneath it, in order.
    #[derive(Clone, Debug, PartialEq)]
    enum FileEvent {
        Write,
        Truncate(u64),
        Sync,
    }

    /// Wraps the real WAL file and fails the operations a test asks for.
    /// The log is shared so a test can inspect it after `Wal` owns the target.
    struct FaultyTarget {
        file: File,
        log: Arc<Mutex<Vec<FileEvent>>>,
        fail_writes: Arc<Mutex<bool>>,
        fail_syncs: Arc<Mutex<bool>>,
        /// Fail only the next sync, then sync normally again.
        fail_one_sync: Arc<Mutex<bool>>,
    }

    impl Write for FaultyTarget {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if *self.fail_writes.lock().unwrap() {
                // Leave torn bytes behind, as a real short write would.
                let torn = buf.len().min(3);
                self.file.write_all(&buf[..torn])?;
                self.log.lock().unwrap().push(FileEvent::Write);
                return Err(io::Error::other("injected write failure"));
            }
            let written = self.file.write(buf)?;
            self.log.lock().unwrap().push(FileEvent::Write);
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl AppendTarget for FaultyTarget {
        fn seek_end(&mut self) -> io::Result<u64> {
            self.file.seek_end()
        }

        fn truncate(&mut self, len: u64) -> io::Result<()> {
            self.file.truncate(len)?;
            self.log.lock().unwrap().push(FileEvent::Truncate(len));
            Ok(())
        }

        fn sync(&mut self) -> io::Result<()> {
            let once = std::mem::take(&mut *self.fail_one_sync.lock().unwrap());
            if once || *self.fail_syncs.lock().unwrap() {
                return Err(io::Error::other("injected sync failure"));
            }
            AppendTarget::sync(&mut self.file)?;
            self.log.lock().unwrap().push(FileEvent::Sync);
            Ok(())
        }
    }

    struct FaultyWal {
        wal: Wal,
        path: PathBuf,
        log: Arc<Mutex<Vec<FileEvent>>>,
        fail_writes: Arc<Mutex<bool>>,
        fail_syncs: Arc<Mutex<bool>>,
        fail_one_sync: Arc<Mutex<bool>>,
        _dir: tempfile::TempDir,
    }

    /// A flush-every WAL holding one acknowledged entry, built through the
    /// same `from_target` path as `with_group_commit`, over a faulty target.
    fn faulty_wal() -> FaultyWal {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wal.bin");
        drop(Wal::new(&path, true).unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let fail_writes = Arc::new(Mutex::new(false));
        let fail_syncs = Arc::new(Mutex::new(false));
        let fail_one_sync = Arc::new(Mutex::new(false));
        let target = FaultyTarget {
            file: open_wal_writer(&path).unwrap(),
            log: Arc::clone(&log),
            fail_writes: Arc::clone(&fail_writes),
            fail_syncs: Arc::clone(&fail_syncs),
            fail_one_sync: Arc::clone(&fail_one_sync),
        };
        let wal = Wal::from_target(
            &path,
            Box::new(target),
            true,
            DEFAULT_GROUP_COMMIT_INTERVAL,
            DEFAULT_GROUP_COMMIT_BATCH_SIZE,
        );
        wal.append(&insert_node("acked")).unwrap();
        log.lock().unwrap().clear();
        FaultyWal {
            wal,
            path,
            log,
            fail_writes,
            fail_syncs,
            fail_one_sync,
            _dir: dir,
        }
    }

    /// Turns a WAL's appends into refusals and back, from outside the WAL.
    pub(crate) struct AppendSwitch(Arc<Mutex<bool>>);

    impl AppendSwitch {
        pub(crate) fn refuse(&self, refuse: bool) {
            *self.0.lock().unwrap() = refuse;
        }
    }

    /// A flush-every WAL over the existing file at `path`, built through the
    /// same `from_target` path as `with_group_commit`. While the switch is on,
    /// each append tears its write and gets the error a failing disk gives.
    pub(crate) fn switchable_wal(path: &Path) -> (Wal, AppendSwitch) {
        let fail_writes = Arc::new(Mutex::new(false));
        let target = FaultyTarget {
            file: open_wal_writer(path).unwrap(),
            log: Arc::new(Mutex::new(Vec::new())),
            fail_writes: Arc::clone(&fail_writes),
            fail_syncs: Arc::new(Mutex::new(false)),
            fail_one_sync: Arc::new(Mutex::new(false)),
        };
        let wal = Wal::from_target(
            path,
            Box::new(target),
            true,
            DEFAULT_GROUP_COMMIT_INTERVAL,
            DEFAULT_GROUP_COMMIT_BATCH_SIZE,
        );
        (wal, AppendSwitch(fail_writes))
    }

    #[test]
    fn failed_append_syncs_the_rollback_before_returning() {
        let faulty = faulty_wal();
        let valid_len = std::fs::metadata(&faulty.path).unwrap().len();
        *faulty.fail_writes.lock().unwrap() = true;

        let error = faulty.wal.append(&insert_node("torn")).unwrap_err();

        assert!(
            matches!(error, WalError::Io(_)),
            "unexpected error: {error}"
        );
        // By the time the error surfaced, the truncate back to the last good
        // length had happened AND been synced, and nothing came after it.
        assert_eq!(
            *faulty.log.lock().unwrap(),
            [
                FileEvent::Write,
                FileEvent::Truncate(valid_len),
                FileEvent::Sync
            ]
        );
        assert_eq!(std::fs::metadata(&faulty.path).unwrap().len(), valid_len);

        // A synced rollback is a clean state: the WAL is not poisoned.
        *faulty.fail_writes.lock().unwrap() = false;
        faulty.wal.append(&insert_node("after")).unwrap();
        assert_eq!(node_labels(&faulty.wal.iter().unwrap()), ["acked", "after"]);
    }

    #[test]
    fn unsynced_rollback_poisons_the_wal() {
        let faulty = faulty_wal();
        let valid_len = std::fs::metadata(&faulty.path).unwrap().len();
        *faulty.fail_writes.lock().unwrap() = true;
        *faulty.fail_syncs.lock().unwrap() = true;

        let error = faulty.wal.append(&insert_node("torn")).unwrap_err();

        let WalError::Durability(message) = &error else {
            panic!("expected a durability error, got {error}");
        };
        assert!(
            message.contains("may be inconsistent"),
            "error must say the WAL may be inconsistent: {message}"
        );
        assert_eq!(
            *faulty.log.lock().unwrap(),
            [FileEvent::Write, FileEvent::Truncate(valid_len)]
        );

        // Even once the disk recovers, the WAL refuses every further write
        // without touching the file, until it is reopened.
        *faulty.fail_writes.lock().unwrap() = false;
        *faulty.fail_syncs.lock().unwrap() = false;
        faulty.log.lock().unwrap().clear();
        for label in ["refused-1", "refused-2"] {
            let refused = faulty.wal.append(&insert_node(label)).unwrap_err();
            assert!(
                matches!(&refused, WalError::Durability(m) if m == message),
                "poisoned WAL accepted or changed its error: {refused}"
            );
        }
        assert!(faulty.log.lock().unwrap().is_empty());
        assert_eq!(std::fs::metadata(&faulty.path).unwrap().len(), valid_len);

        // Reopening replays only what was acknowledged.
        let path = faulty.path.clone();
        drop(faulty.wal);
        let reopened = Wal::new(&path, true).unwrap();
        assert_eq!(node_labels(&reopened.iter().unwrap()), ["acked"]);
        reopened.append(&insert_node("after-reopen")).unwrap();
        assert_eq!(
            node_labels(&reopened.iter().unwrap()),
            ["acked", "after-reopen"]
        );
    }

    #[test]
    fn failed_sync_removes_the_unsynced_entry_and_poisons_the_wal() {
        let faulty = faulty_wal();
        let valid_len = std::fs::metadata(&faulty.path).unwrap().len();
        *faulty.fail_one_sync.lock().unwrap() = true;

        let error = faulty.wal.append(&insert_node("unsynced")).unwrap_err();

        let WalError::Durability(message) = &error else {
            panic!("expected a durability error, got {error}");
        };
        assert!(message.contains("unsynced writes were removed"), "{message}");
        // The caller was told the write failed, so it is gone from the file
        // (durably) before the error surfaced, and a reopen cannot replay it.
        assert_eq!(
            *faulty.log.lock().unwrap(),
            [
                FileEvent::Write,
                FileEvent::Write,
                FileEvent::Write,
                FileEvent::Truncate(valid_len),
                FileEvent::Sync
            ]
        );
        assert_eq!(std::fs::metadata(&faulty.path).unwrap().len(), valid_len);
        let refused = faulty.wal.append(&insert_node("later")).unwrap_err();
        assert!(matches!(&refused, WalError::Durability(m) if m == message), "{refused}");

        let path = faulty.path.clone();
        drop(faulty.wal);
        let reopened = Wal::new(&path, true).unwrap();
        assert_eq!(node_labels(&reopened.iter().unwrap()), ["acked"]);
    }

    #[test]
    fn failed_group_commit_sync_removes_every_waiting_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wal.bin");
        drop(Wal::new(&path, true).unwrap());
        let valid_len = std::fs::metadata(&path).unwrap().len();
        let fail_one_sync = Arc::new(Mutex::new(true));
        let target = FaultyTarget {
            file: open_wal_writer(&path).unwrap(),
            log: Arc::new(Mutex::new(Vec::new())),
            fail_writes: Arc::new(Mutex::new(false)),
            fail_syncs: Arc::new(Mutex::new(false)),
            fail_one_sync,
        };
        // Batch of 2, long interval: both appends wait on the same sync.
        let wal = Arc::new(Wal::from_target(
            &path,
            Box::new(target),
            false,
            Duration::from_secs(60),
            2,
        ));
        let writers: Vec<_> = ["one", "two"]
            .into_iter()
            .map(|label| {
                let wal = Arc::clone(&wal);
                thread::spawn(move || wal.append(&insert_node(label)))
            })
            .collect();
        for writer in writers {
            let error = writer.join().unwrap().unwrap_err();
            assert!(matches!(error, WalError::Durability(_)), "{error}");
        }
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_len);
        drop(Arc::try_unwrap(wal).ok().unwrap());
        assert!(Wal::new(&path, true).unwrap().iter().unwrap().is_empty());
    }

    #[test]
    fn failed_sync_whose_removal_cannot_be_synced_says_so() {
        let faulty = faulty_wal();
        let valid_len = std::fs::metadata(&faulty.path).unwrap().len();
        *faulty.fail_syncs.lock().unwrap() = true;

        let error = faulty.wal.append(&insert_node("unsynced")).unwrap_err();

        let WalError::Durability(message) = &error else {
            panic!("expected a durability error, got {error}");
        };
        assert!(message.contains("may be inconsistent"), "{message}");
        assert_eq!(std::fs::metadata(&faulty.path).unwrap().len(), valid_len);
        *faulty.fail_syncs.lock().unwrap() = false;
        assert!(faulty.wal.append(&insert_node("later")).is_err());
    }

    #[test]
    fn a_statement_is_one_entry_and_a_torn_one_replays_as_nothing() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let wal = Wal::new(&wal_path, true).unwrap();
        wal.append_statement(vec![insert_node("acked")]).unwrap();
        let acked_len = std::fs::metadata(&wal_path).unwrap().len();
        wal.append_statement(vec![insert_node("one"), insert_node("two")])
            .unwrap();
        wal.append_statement(Vec::new()).unwrap();
        let ops = wal.iter().unwrap();
        assert_eq!(ops.len(), 2, "one entry per statement, none for an empty one");
        let Operation::Statement { ops: inner } = &ops[1] else {
            panic!("expected a statement entry, got {:?}", ops[1]);
        };
        assert_eq!(node_labels(inner), ["one", "two"]);
        drop(wal);

        // A crash inside the statement entry, even after its first operation's
        // bytes, loses the whole statement and nothing before it.
        let full_len = std::fs::metadata(&wal_path).unwrap().len();
        let file = OpenOptions::new().write(true).open(&wal_path).unwrap();
        file.set_len(full_len - 1).unwrap();
        drop(file);
        let wal = Wal::new(&wal_path, true).unwrap();
        assert_eq!(node_labels(&wal.iter().unwrap()), ["acked"]);
        assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), acked_len);
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

    /// zega#44: a frame whose CRC is right but whose payload is one
    /// operation followed by more bytes was not written by any writer.
    fn append_padded_entry(wal_path: &Path, op: &Operation) -> u64 {
        let offset = std::fs::metadata(wal_path).unwrap().len();
        let mut payload = bincode::serialize(op).unwrap();
        payload.extend_from_slice(&[0xAB, 0xCD]);
        let mut file = OpenOptions::new().append(true).open(wal_path).unwrap();
        file.write_all(&(payload.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&crc32fast::hash(&payload).to_le_bytes()).unwrap();
        file.write_all(&payload).unwrap();
        offset
    }

    #[test]
    fn an_entry_with_bytes_after_its_operation_is_corruption_at_its_offset() {
        // In the middle of the log and as its last entry: the CRC is valid,
        // so this is not a torn tail to drop but a frame to refuse.
        for last in [false, true] {
            let dir = tempdir().unwrap();
            let wal_path = dir.path().join("wal.bin");
            let wal = Wal::new(&wal_path, true).unwrap();
            wal.append(&insert_node("first")).unwrap();
            drop(wal);
            let offset = append_padded_entry(&wal_path, &insert_node("padded"));
            if !last {
                let wal = Wal::new(&wal_path, true).unwrap();
                wal.append(&insert_node("after")).unwrap();
            }
            let len = std::fs::metadata(&wal_path).unwrap().len();

            let wal = Wal::new(&wal_path, false).unwrap();
            match wal.iter() {
                Err(WalError::Corruption { offset: at, reason }) => {
                    assert_eq!(at, offset, "last={last}: {reason}");
                    assert!(reason.contains("invalid operation payload"), "{reason}");
                }
                other => panic!("last={last}: expected corruption at {offset}, got {other:?}"),
            }
            // Refused, not repaired: the file is left for inspection.
            assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), len, "last={last}");
        }
    }

    #[test]
    fn a_legacy_entry_with_bytes_after_its_operation_is_corruption_at_its_offset() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("wal.bin");
        let mut legacy = File::create(&wal_path).unwrap();
        let exact = bincode::serialize(&insert_node("exact")).unwrap();
        legacy.write_all(&(exact.len() as u64).to_le_bytes()).unwrap();
        legacy.write_all(&exact).unwrap();
        let mut padded = bincode::serialize(&insert_node("padded")).unwrap();
        padded.push(0xAB);
        legacy.write_all(&(padded.len() as u64).to_le_bytes()).unwrap();
        legacy.write_all(&padded).unwrap();
        drop(legacy);

        match Wal::new(&wal_path, true) {
            Err(WalError::Corruption { offset, reason }) => {
                assert_eq!(offset, 8 + exact.len() as u64, "{reason}");
                assert!(reason.contains("invalid legacy operation payload"), "{reason}");
            }
            Err(other) => panic!("expected corruption, got {other:?}"),
            Ok(_) => panic!("expected corruption, got a migrated WAL"),
        }
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
