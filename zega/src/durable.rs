//! Target-neutral host journal and checkpoint API.
use super::*;

impl Zega {
    /// Construct an engine whose journal appends to synchronous host storage.
    /// The host owns transaction boundaries, recovery and the durability gate.
    pub fn with_append_target(target: Box<dyn AppendTarget + Send>) -> Result<Self> {
        let mut db = Self::in_memory().build()?;
        db.wal = Wal::with_append_target(target);
        Ok(db)
    }

    /// Replay one exact WAL frame without logging it again.
    pub fn replay_wal_entry(&self, frame: &[u8]) -> Result<()> {
        let op = crate::wal::decode_entry(frame)?;
        let mut graph = self.graph.lock()
            .map_err(|_| ZegaError::Execution("lock poisoned".into()))?;
        apply_op_to_memory(&mut graph, &op);
        Ok(())
    }

    /// Host checkpoint metadata: snapshots predate persistent id counters.
    /// Store this alongside the snapshot, in the same transaction.
    pub fn checkpoint_ids(&self) -> Result<(u64, u64)> {
        Ok(self.graph.lock().map_err(|_| ZegaError::Execution("lock poisoned".into()))?.next_ids())
    }

    /// Restore checkpoint metadata before replaying entries after a snapshot.
    /// Refuse counters that could reuse a live id.
    pub fn restore_checkpoint_ids(&self, ids: (u64, u64)) -> Result<()> {
        let mut graph = self.graph.lock().map_err(|_| ZegaError::Execution("lock poisoned".into()))?;
        let minimum = graph.next_ids();
        if ids.0 < minimum.0 || ids.1 < minimum.1 {
            return Err(ZegaError::Execution("checkpoint id counters precede live ids".into()));
        }
        graph.reset_next_ids(ids);
        Ok(())
    }

}
