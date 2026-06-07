use std::sync::Arc;
use std::thread;
use zega_kv::KvStore;
use zega_parser::Value;

#[test]
fn zero_ttl_value_is_never_observable_during_concurrent_sets() {
    let kv = Arc::new(KvStore::new());
    kv.set("expired".into(), Value::Int(0), Some(0));

    let writer = Arc::clone(&kv);
    let writer = thread::spawn(move || {
        for value in 1..=1_000_000 {
            writer.set("expired".into(), Value::Int(value), Some(0));
        }
    });

    for _ in 0..1_000_000 {
        assert_eq!(kv.get("expired"), None);
    }

    writer.join().unwrap();
}
