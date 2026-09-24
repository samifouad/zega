use std::{
    collections::HashMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};
use zega::{AppendTarget, Zega, ZqlProgram};
const SCHEMA: &str = "schema { type Item { key: String value: Int } } unique { Item { key } }";
const LOAD: &str = "mutation json [\"./rows.json\"] { Item(key: $key && value: $value) { key } }";
fn sources() -> HashMap<String, String> {
    HashMap::from([(
        "./rows.json".into(),
        r#"[{"key":"a","value":1},{"key":"b","value":2}]"#.into(),
    )])
}
#[derive(Default)]
struct Rows {
    frames: Vec<Vec<u8>>,
    fail: bool,
}
struct Target(Arc<Mutex<Rows>>);
impl Write for Target {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut rows = self.0.lock().unwrap();
        rows.frames.push(bytes.to_vec());
        if rows.fail {
            return Err(io::Error::other("injected append failure"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl AppendTarget for Target {
    fn write_entry(&mut self, header: &[u8; 12], payload: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(header.len() + payload.len());
        frame.extend_from_slice(header);
        frame.extend_from_slice(payload);
        self.write_all(&frame)
    }

    fn seek_end(&mut self) -> io::Result<u64> {
        Ok(self.0.lock().unwrap().frames.len() as u64)
    }
    fn truncate(&mut self, seq: u64) -> io::Result<()> {
        self.0.lock().unwrap().frames.truncate(seq as usize);
        Ok(())
    }
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn native_frames_replay_atomically_and_refused_block_restores_graph() {
    let rows = Arc::new(Mutex::new(Rows::default()));
    let db = Zega::with_append_target(Box::new(Target(rows.clone()))).unwrap();
    db.run_lang_with_sources(SCHEMA, LOAD, &sources()).unwrap();
    assert_eq!(rows.lock().unwrap().frames.len(), 1);
    let before = db.graph_json().unwrap();
    let restored = Zega::in_memory().build().unwrap();
    restored
        .replay_wal_entry(&rows.lock().unwrap().frames[0])
        .unwrap();
    assert_eq!(restored.graph_json().unwrap(), before);
    rows.lock().unwrap().fail = true;
    assert!(db
        .run_lang(SCHEMA, "mutation { Item(key: \"c\" && value: 3) { key } }")
        .is_err());
    assert_eq!(db.graph_json().unwrap(), before);
    assert_eq!(rows.lock().unwrap().frames.len(), 1);
    assert_eq!(
        db.checkpoint_ids().unwrap(),
        restored.checkpoint_ids().unwrap()
    );
    let mut corrupt = rows.lock().unwrap().frames[0].clone();
    let end = corrupt.len() - 1;
    corrupt[end] ^= 1;
    assert!(restored.replay_wal_entry(&corrupt).is_err());
    assert_eq!(restored.graph_json().unwrap(), before);
}
#[test]
fn host_program_keeps_successful_block_when_later_block_fails() {
    let source = format!("{SCHEMA}\nmutation {{ Item(key: \"a\" && value: 1) {{ key }} }}\n{LOAD}");
    let program = ZqlProgram::parse("", &source, true).unwrap();
    let db = Zega::in_memory().build().unwrap();
    program.execute(&db, 0, &sources()).unwrap();
    let before = db.graph_json().unwrap();
    assert!(program.execute(&db, 1, &sources()).is_err());
    assert_eq!(db.graph_json().unwrap(), before);
    let native = Zega::in_memory().build().unwrap();
    assert!(native.apply_zql_with_sources(&source, &sources()).is_err());
    assert_eq!(native.graph_json().unwrap(), before);
}
#[test]
fn snapshot_metadata_preserves_deleted_highest_ids() {
    let db = Zega::in_memory().build().unwrap();
    db.run_lang_with_sources(SCHEMA, LOAD, &sources()).unwrap();
    let graph = db.graph_json().unwrap();
    let highest = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_u64().unwrap())
        .max()
        .unwrap();
    db.delete_node(highest).unwrap();
    let ids = db.checkpoint_ids().unwrap();
    let recovered = Zega::in_memory().build().unwrap();
    recovered
        .restore_bytes(&db.snapshot_bytes().unwrap())
        .unwrap();
    recovered.restore_checkpoint_ids(ids).unwrap();
    assert_eq!(recovered.checkpoint_ids().unwrap(), ids);
    assert_eq!(recovered.graph_json().unwrap(), db.graph_json().unwrap());
}
