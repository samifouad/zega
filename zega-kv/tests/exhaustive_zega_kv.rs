//! Exhaustive integration tests for the `zega-kv` crate.
//!
//! Targets the public API of `KvStore` plus the `KvEntry` snapshot type.
//! Semantics mirror Redis: missing-key reads are absent, overwrite replaces,
//! `incr` auto-creates, lists are head-pushed, and TTL'd keys read ABSENT once
//! the clock advances past their expiry (lazy expiry on read).
//!
//! Notes on intentionally-uncovered behavior:
//!   * `incr` performs `n + 1` with no checked-add, so calling it on
//!     `Value::Int(i64::MAX)` would panic in debug builds (overflow). Per the
//!     "errors are values" rule we do NOT exercise that path — it is a real
//!     overflow footgun, not a Result we can assert on.
//!   * The background `start_eviction_task` spawns a real OS thread + tokio
//!     runtime and only acts on whole-second boundaries; deterministic
//!     assertions there are timing-flaky, so we assert it is callable and that
//!     lazy-expiry (the deterministic path) works, rather than racing the timer.
//!   * `lrange`/`ltrim` clamp `start`/`stop` independently to `len` but do NOT
//!     reorder them, so a call with `start > stop` (e.g. `lrange(k, 2, 1)` on a
//!     non-empty list) computes `items[s..e]` with `s > e`, which panics on the
//!     slice. Per the "no expected panics / errors are values" rule we do NOT
//!     exercise that inverted-range path — it is a real footgun, not a Result.
//!     We DO cover every non-inverted clamp boundary.

use std::collections::HashMap;

use zega_kv::{KvEntry, KvStore};
use zega_parser::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

// ===========================================================================
// SET / GET — basic round-trips and value types
// ===========================================================================

#[test]
fn set_then_get_returns_string_value() {
    let kv = KvStore::new();
    kv.set("k".to_string(), s("hello"), None);
    assert_eq!(kv.get("k"), Some(s("hello")));
}

#[test]
fn set_then_get_returns_int_value() {
    let kv = KvStore::new();
    kv.set("n".to_string(), Value::Int(42), None);
    assert_eq!(kv.get("n"), Some(Value::Int(42)));
}

#[test]
fn set_then_get_returns_negative_int_value() {
    let kv = KvStore::new();
    kv.set("n".to_string(), Value::Int(-99), None);
    assert_eq!(kv.get("n"), Some(Value::Int(-99)));
}

#[test]
fn set_then_get_returns_int_min_and_max() {
    let kv = KvStore::new();
    kv.set("min".to_string(), Value::Int(i64::MIN), None);
    kv.set("max".to_string(), Value::Int(i64::MAX), None);
    assert_eq!(kv.get("min"), Some(Value::Int(i64::MIN)));
    assert_eq!(kv.get("max"), Some(Value::Int(i64::MAX)));
}

#[test]
fn set_then_get_returns_bool_values() {
    let kv = KvStore::new();
    kv.set("t".to_string(), Value::Bool(true), None);
    kv.set("f".to_string(), Value::Bool(false), None);
    assert_eq!(kv.get("t"), Some(Value::Bool(true)));
    assert_eq!(kv.get("f"), Some(Value::Bool(false)));
}

#[test]
fn set_then_get_returns_float_value() {
    let kv = KvStore::new();
    let f = Value::from_f64(3.14159);
    kv.set("pi".to_string(), f.clone(), None);
    assert_eq!(kv.get("pi"), Some(f));
    assert_eq!(kv.get("pi").and_then(|v| v.to_f64()), Some(3.14159));
}

#[test]
fn set_then_get_returns_null_value() {
    let kv = KvStore::new();
    // Storing an explicit Null is distinct from a missing key.
    kv.set("nothing".to_string(), Value::Null, None);
    assert_eq!(kv.get("nothing"), Some(Value::Null));
}

#[test]
fn set_then_get_returns_list_value() {
    let kv = KvStore::new();
    let list = Value::List(vec![Value::Int(1), s("two"), Value::Bool(true)]);
    kv.set("l".to_string(), list.clone(), None);
    assert_eq!(kv.get("l"), Some(list));
}

#[test]
fn set_then_get_returns_map_value() {
    let kv = KvStore::new();
    let mut m = HashMap::new();
    m.insert("a".to_string(), Value::Int(1));
    m.insert("b".to_string(), s("x"));
    let map = Value::Map(m);
    kv.set("m".to_string(), map.clone(), None);
    assert_eq!(kv.get("m"), Some(map));
}

// ===========================================================================
// GET — missing key semantics
// ===========================================================================

#[test]
fn get_missing_key_is_none() {
    let kv = KvStore::new();
    assert_eq!(kv.get("does-not-exist"), None);
}

#[test]
fn get_missing_key_is_none_distinct_from_stored_null() {
    let kv = KvStore::new();
    // A missing key reads None; a stored Null reads Some(Null). They differ.
    assert_eq!(kv.get("absent"), None);
    kv.set("present".to_string(), Value::Null, None);
    assert_eq!(kv.get("present"), Some(Value::Null));
    assert_ne!(kv.get("absent"), kv.get("present"));
}

#[test]
fn get_after_del_is_none() {
    let kv = KvStore::new();
    kv.set("x".to_string(), Value::Int(7), None);
    assert!(kv.del("x"));
    assert_eq!(kv.get("x"), None);
}

// ===========================================================================
// SET — overwrite semantics
// ===========================================================================

#[test]
fn set_overwrites_existing_value() {
    let kv = KvStore::new();
    kv.set("k".to_string(), s("first"), None);
    kv.set("k".to_string(), s("second"), None);
    assert_eq!(kv.get("k"), Some(s("second")));
}

#[test]
fn set_overwrites_changing_value_type() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    kv.set("k".to_string(), s("now-a-string"), None);
    assert_eq!(kv.get("k"), Some(s("now-a-string")));
}

#[test]
fn set_overwrite_clears_previous_ttl_when_set_without_ttl() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), Some(10_000));
    // Re-set without TTL: the entry is fully replaced, so no expiry remains.
    kv.set("k".to_string(), Value::Int(2), None);
    let snap = kv.snapshot();
    assert_eq!(snap.get("k").map(|e| e.expires_at), Some(None));
}

#[test]
fn set_overwrite_replaces_ttl_with_new_ttl() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    kv.set("k".to_string(), Value::Int(2), Some(10_000));
    let snap = kv.snapshot();
    // The new entry now carries an expiry far in the future.
    let exp = snap.get("k").and_then(|e| e.expires_at);
    assert!(exp.is_some());
}

// ===========================================================================
// DEL — return value semantics (Redis returns count removed)
// ===========================================================================

#[test]
fn del_existing_key_returns_true() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    assert!(kv.del("k"));
}

#[test]
fn del_missing_key_returns_false() {
    let kv = KvStore::new();
    assert!(!kv.del("nope"));
}

#[test]
fn del_twice_second_returns_false() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    assert!(kv.del("k"));
    assert!(!kv.del("k"));
}

// ===========================================================================
// INCR — auto-create, increment, type rejection
// ===========================================================================

#[test]
fn incr_missing_key_creates_one() {
    let kv = KvStore::new();
    assert_eq!(kv.incr("c"), Some(Value::Int(1)));
}

#[test]
fn incr_increments_existing_counter() {
    let kv = KvStore::new();
    assert_eq!(kv.incr("c"), Some(Value::Int(1)));
    assert_eq!(kv.incr("c"), Some(Value::Int(2)));
    assert_eq!(kv.incr("c"), Some(Value::Int(3)));
}

#[test]
fn incr_persists_value_observable_via_get() {
    let kv = KvStore::new();
    kv.incr("c");
    kv.incr("c");
    assert_eq!(kv.get("c"), Some(Value::Int(2)));
}

#[test]
fn incr_on_preset_int_increments_it() {
    let kv = KvStore::new();
    kv.set("c".to_string(), Value::Int(40), None);
    assert_eq!(kv.incr("c"), Some(Value::Int(41)));
    assert_eq!(kv.incr("c"), Some(Value::Int(42)));
}

#[test]
fn incr_from_negative_crosses_zero() {
    let kv = KvStore::new();
    kv.set("c".to_string(), Value::Int(-2), None);
    assert_eq!(kv.incr("c"), Some(Value::Int(-1)));
    assert_eq!(kv.incr("c"), Some(Value::Int(0)));
    assert_eq!(kv.incr("c"), Some(Value::Int(1)));
}

#[test]
fn incr_on_string_value_returns_none() {
    let kv = KvStore::new();
    kv.set("c".to_string(), s("not-a-number"), None);
    assert_eq!(kv.incr("c"), None);
    // Value is left unchanged.
    assert_eq!(kv.get("c"), Some(s("not-a-number")));
}

#[test]
fn incr_on_bool_value_returns_none() {
    let kv = KvStore::new();
    kv.set("c".to_string(), Value::Bool(true), None);
    assert_eq!(kv.incr("c"), None);
}

#[test]
fn incr_on_float_value_returns_none() {
    let kv = KvStore::new();
    kv.set("c".to_string(), Value::from_f64(1.5), None);
    assert_eq!(kv.incr("c"), None);
}

#[test]
fn incr_on_list_value_returns_none() {
    let kv = KvStore::new();
    kv.set("c".to_string(), Value::List(vec![Value::Int(1)]), None);
    assert_eq!(kv.incr("c"), None);
}

#[test]
fn incr_on_null_value_returns_none() {
    let kv = KvStore::new();
    kv.set("c".to_string(), Value::Null, None);
    assert_eq!(kv.incr("c"), None);
}

#[test]
fn incr_auto_created_counter_has_no_ttl() {
    let kv = KvStore::new();
    kv.incr("c");
    let snap = kv.snapshot();
    assert_eq!(snap.get("c").map(|e| e.expires_at), Some(None));
}

// ===========================================================================
// TTL / EXPIRY — lazy expiry semantics
// ===========================================================================

#[test]
fn set_with_no_ttl_records_no_expiry() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    let snap = kv.snapshot();
    assert_eq!(snap.get("k").and_then(|e| e.expires_at), None);
}

#[test]
fn set_with_ttl_records_future_expiry() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), Some(3600));
    let snap = kv.snapshot();
    let exp = snap
        .get("k")
        .and_then(|e| e.expires_at)
        .expect("ttl entry should carry expires_at");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Expiry is roughly now + ttl (allow a small slack for clock read timing).
    assert!(exp >= now + 3590 && exp <= now + 3601, "exp={exp} now={now}");
}

#[test]
fn get_with_long_ttl_still_readable() {
    let kv = KvStore::new();
    kv.set("k".to_string(), s("alive"), Some(10_000));
    // Well within the TTL window — must still read present.
    assert_eq!(kv.get("k"), Some(s("alive")));
}

#[test]
fn get_past_ttl_reads_absent() {
    // Inject an entry whose expiry is in the past via restore(), then confirm
    // a read lazily evicts it and returns None (Redis: expired key is absent).
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "stale".to_string(),
        KvEntry {
            value: s("old"),
            expires_at: Some(1), // 1 second after the epoch — long past.
        },
    );
    kv.restore(entries);
    assert_eq!(kv.get("stale"), None);
}

#[test]
fn get_lazily_removes_expired_entry_from_store() {
    // After a read on an expired key, the entry should be physically gone.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "stale".to_string(),
        KvEntry {
            value: Value::Int(5),
            expires_at: Some(1),
        },
    );
    kv.restore(entries);
    // Trigger lazy expiry.
    assert_eq!(kv.get("stale"), None);
    // It is now absent from the snapshot too.
    assert!(!kv.snapshot().contains_key("stale"));
}

#[test]
fn get_does_not_evict_unexpired_entry() {
    let kv = KvStore::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut entries = HashMap::new();
    entries.insert(
        "fresh".to_string(),
        KvEntry {
            value: Value::Int(5),
            expires_at: Some(now + 10_000),
        },
    );
    kv.restore(entries);
    assert_eq!(kv.get("fresh"), Some(Value::Int(5)));
    assert!(kv.snapshot().contains_key("fresh"));
}

#[test]
fn del_on_expired_but_not_yet_read_key_still_removes_row() {
    // del does not check expiry; it just removes whatever row exists.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "stale".to_string(),
        KvEntry {
            value: Value::Int(5),
            expires_at: Some(1),
        },
    );
    kv.restore(entries);
    // The physical row exists, so del returns true.
    assert!(kv.del("stale"));
    assert!(!kv.del("stale"));
}

#[test]
fn start_eviction_task_is_callable() {
    // We do not race the 1-second timer; just confirm the API is callable and
    // does not disturb existing data immediately.
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    kv.start_eviction_task();
    assert_eq!(kv.get("k"), Some(Value::Int(1)));
}

// ===========================================================================
// EDGE VALUES — empty, large, unicode
// ===========================================================================

#[test]
fn set_get_empty_string_value() {
    let kv = KvStore::new();
    kv.set("k".to_string(), s(""), None);
    assert_eq!(kv.get("k"), Some(s("")));
}

#[test]
fn set_get_empty_string_key() {
    let kv = KvStore::new();
    kv.set(String::new(), s("v"), None);
    assert_eq!(kv.get(""), Some(s("v")));
    assert!(kv.del(""));
}

#[test]
fn set_get_large_string_value() {
    let kv = KvStore::new();
    let big = "x".repeat(1_000_000);
    kv.set("big".to_string(), s(&big), None);
    assert_eq!(kv.get("big"), Some(s(&big)));
}

#[test]
fn set_get_unicode_string_value() {
    let kv = KvStore::new();
    let u = "héllo-世界-🚀-Ω";
    kv.set("u".to_string(), s(u), None);
    assert_eq!(kv.get("u"), Some(s(u)));
}

#[test]
fn set_get_unicode_key() {
    let kv = KvStore::new();
    kv.set("ключ-🔑".to_string(), Value::Int(1), None);
    assert_eq!(kv.get("ключ-🔑"), Some(Value::Int(1)));
}

#[test]
fn keys_with_whitespace_and_control_chars_are_distinct() {
    let kv = KvStore::new();
    kv.set("a b".to_string(), Value::Int(1), None);
    kv.set("a\tb".to_string(), Value::Int(2), None);
    kv.set("a\nb".to_string(), Value::Int(3), None);
    assert_eq!(kv.get("a b"), Some(Value::Int(1)));
    assert_eq!(kv.get("a\tb"), Some(Value::Int(2)));
    assert_eq!(kv.get("a\nb"), Some(Value::Int(3)));
}

#[test]
fn set_get_large_list_value() {
    let kv = KvStore::new();
    let items: Vec<Value> = (0..10_000).map(Value::Int).collect();
    kv.set("l".to_string(), Value::List(items.clone()), None);
    assert_eq!(kv.get("l"), Some(Value::List(items)));
}

// ===========================================================================
// LPUSH / LRANGE — head insertion and range read
// ===========================================================================

#[test]
fn lpush_creates_list_when_missing() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![Value::Int(1)]));
}

#[test]
fn lpush_inserts_at_head() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    kv.lpush("l", Value::Int(3));
    // Most-recently pushed is at index 0.
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(3), Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn lpush_replaces_non_list_value_with_single_element_list() {
    // Pushing onto a key holding a non-list resets it to a one-element list.
    let kv = KvStore::new();
    kv.set("l".to_string(), s("scalar"), None);
    kv.lpush("l", Value::Int(9));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![Value::Int(9)]));
}

#[test]
fn lpush_mixed_value_types() {
    let kv = KvStore::new();
    kv.lpush("l", s("a"));
    kv.lpush("l", Value::Bool(true));
    kv.lpush("l", Value::Null);
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Null, Value::Bool(true), s("a")])
    );
}

#[test]
fn lrange_on_missing_key_is_none() {
    let kv = KvStore::new();
    assert_eq!(kv.lrange("nope", 0, 10), None);
}

#[test]
fn lrange_on_non_list_value_is_none() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(5), None);
    assert_eq!(kv.lrange("k", 0, 10), None);
}

#[test]
fn lrange_full_window_returns_all() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    assert_eq!(
        kv.lrange("l", 0, 2),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn lrange_partial_window() {
    let kv = KvStore::new();
    // List becomes [3,2,1] after three head-pushes.
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    kv.lpush("l", Value::Int(3));
    // stop is exclusive: [1..2) -> index 1 only.
    assert_eq!(kv.lrange("l", 1, 2), Some(vec![Value::Int(2)]));
}

#[test]
fn lrange_start_equals_stop_returns_empty() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    assert_eq!(kv.lrange("l", 0, 0), Some(vec![]));
}

#[test]
fn lrange_start_beyond_len_returns_empty() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    // start clamps to len; s == e -> empty slice.
    assert_eq!(kv.lrange("l", 50, 100), Some(vec![]));
}

#[test]
fn lrange_stop_beyond_len_clamps_to_len() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    assert_eq!(
        kv.lrange("l", 0, 999),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn lrange_on_empty_list_returns_empty() {
    // Restore a key holding an explicitly empty list.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![]),
            expires_at: None,
        },
    );
    kv.restore(entries);
    assert_eq!(kv.lrange("l", 0, 10), Some(vec![]));
}

// ===========================================================================
// LTRIM — in-place trimming
// ===========================================================================

#[test]
fn ltrim_keeps_requested_window() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    kv.lpush("l", Value::Int(3)); // [3,2,1]
    assert!(kv.ltrim("l", 0, 2));
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(3), Value::Int(2)])
    );
}

#[test]
fn ltrim_to_single_element() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2)); // [2,1]
    assert!(kv.ltrim("l", 0, 1));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![Value::Int(2)]));
}

#[test]
fn ltrim_to_empty_when_start_equals_stop() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    assert!(kv.ltrim("l", 0, 0));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![]));
}

#[test]
fn ltrim_clamps_out_of_range_bounds() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2)); // [2,1]
    // stop beyond len clamps to len; whole list retained.
    assert!(kv.ltrim("l", 0, 999));
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn ltrim_start_beyond_len_yields_empty_list() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    assert!(kv.ltrim("l", 10, 20));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![]));
}

#[test]
fn ltrim_missing_key_returns_false() {
    let kv = KvStore::new();
    assert!(!kv.ltrim("nope", 0, 1));
}

#[test]
fn ltrim_non_list_value_returns_false() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(5), None);
    assert!(!kv.ltrim("k", 0, 1));
    // Value untouched.
    assert_eq!(kv.get("k"), Some(Value::Int(5)));
}

// ===========================================================================
// PUB/SUB
// ===========================================================================

#[test]
fn publish_with_no_subscribers_returns_zero() {
    let kv = KvStore::new();
    assert_eq!(kv.publish("chan", Value::Int(1)), 0);
}

#[test]
fn publish_to_one_subscriber_returns_one_and_delivers() {
    let kv = KvStore::new();
    let mut rx = kv.subscribe("chan");
    let n = kv.publish("chan", s("hi"));
    assert_eq!(n, 1);
    assert_eq!(rx.try_recv().ok(), Some(s("hi")));
}

#[test]
fn publish_to_multiple_subscribers_returns_count() {
    let kv = KvStore::new();
    let mut rx1 = kv.subscribe("chan");
    let mut rx2 = kv.subscribe("chan");
    let n = kv.publish("chan", Value::Int(7));
    assert_eq!(n, 2);
    assert_eq!(rx1.try_recv().ok(), Some(Value::Int(7)));
    assert_eq!(rx2.try_recv().ok(), Some(Value::Int(7)));
}

#[test]
fn publish_to_different_channel_is_not_delivered() {
    let kv = KvStore::new();
    let mut rx = kv.subscribe("a");
    let n = kv.publish("b", Value::Int(1));
    assert_eq!(n, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn subscribe_creates_channel_so_later_publish_counts() {
    let kv = KvStore::new();
    // First publish has no subscribers.
    assert_eq!(kv.publish("c", Value::Int(0)), 0);
    let _rx = kv.subscribe("c");
    // Now the channel exists with one live receiver.
    assert_eq!(kv.publish("c", Value::Int(1)), 1);
}

#[test]
fn multiple_messages_delivered_in_order() {
    let kv = KvStore::new();
    let mut rx = kv.subscribe("c");
    kv.publish("c", Value::Int(1));
    kv.publish("c", Value::Int(2));
    kv.publish("c", Value::Int(3));
    assert_eq!(rx.try_recv().ok(), Some(Value::Int(1)));
    assert_eq!(rx.try_recv().ok(), Some(Value::Int(2)));
    assert_eq!(rx.try_recv().ok(), Some(Value::Int(3)));
}

// ===========================================================================
// SNAPSHOT / RESTORE
// ===========================================================================

#[test]
fn snapshot_of_empty_store_is_empty() {
    let kv = KvStore::new();
    assert!(kv.snapshot().is_empty());
}

#[test]
fn snapshot_captures_all_entries_and_values() {
    let kv = KvStore::new();
    kv.set("a".to_string(), Value::Int(1), None);
    kv.set("b".to_string(), s("two"), None);
    let snap = kv.snapshot();
    assert_eq!(snap.len(), 2);
    assert_eq!(snap.get("a").map(|e| e.value.clone()), Some(Value::Int(1)));
    assert_eq!(snap.get("b").map(|e| e.value.clone()), Some(s("two")));
}

#[test]
fn snapshot_preserves_ttl_metadata() {
    let kv = KvStore::new();
    kv.set("a".to_string(), Value::Int(1), Some(5000));
    let snap = kv.snapshot();
    assert!(snap.get("a").and_then(|e| e.expires_at).is_some());
}

#[test]
fn restore_replaces_existing_contents() {
    let kv = KvStore::new();
    kv.set("old".to_string(), Value::Int(1), None);
    let mut entries = HashMap::new();
    entries.insert(
        "new".to_string(),
        KvEntry {
            value: Value::Int(2),
            expires_at: None,
        },
    );
    kv.restore(entries);
    // Old key cleared, new key present.
    assert_eq!(kv.get("old"), None);
    assert_eq!(kv.get("new"), Some(Value::Int(2)));
}

#[test]
fn restore_with_empty_map_clears_store() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    kv.restore(HashMap::new());
    assert!(kv.snapshot().is_empty());
    assert_eq!(kv.get("k"), None);
}

#[test]
fn snapshot_then_restore_round_trips() {
    let kv = KvStore::new();
    kv.set("a".to_string(), Value::Int(1), None);
    kv.set("b".to_string(), Value::List(vec![s("x"), s("y")]), None);
    let snap = kv.snapshot();

    let other = KvStore::new();
    other.restore(snap);
    assert_eq!(other.get("a"), Some(Value::Int(1)));
    assert_eq!(other.get("b"), Some(Value::List(vec![s("x"), s("y")])));
}

// ===========================================================================
// PER-INSTANCE ISOLATION
// ===========================================================================

#[test]
fn two_stores_have_independent_data() {
    let a = KvStore::new();
    let b = KvStore::new();
    a.set("k".to_string(), Value::Int(1), None);
    assert_eq!(a.get("k"), Some(Value::Int(1)));
    assert_eq!(b.get("k"), None);
}

#[test]
fn two_stores_independent_counters() {
    let a = KvStore::new();
    let b = KvStore::new();
    a.incr("c");
    a.incr("c");
    b.incr("c");
    assert_eq!(a.get("c"), Some(Value::Int(2)));
    assert_eq!(b.get("c"), Some(Value::Int(1)));
}

#[test]
fn two_stores_independent_pubsub_channels() {
    let a = KvStore::new();
    let b = KvStore::new();
    let _rx = a.subscribe("chan");
    // b has no subscriber on "chan", so its publish reaches nobody.
    assert_eq!(b.publish("chan", Value::Int(1)), 0);
    assert_eq!(a.publish("chan", Value::Int(1)), 1);
}

#[test]
fn default_constructs_empty_store_equivalent_to_new() {
    let kv = KvStore::default();
    assert!(kv.snapshot().is_empty());
    kv.set("k".to_string(), Value::Int(1), None);
    assert_eq!(kv.get("k"), Some(Value::Int(1)));
}

// ===========================================================================
// MIXED-OPERATION SCENARIOS
// ===========================================================================

#[test]
fn set_del_reset_cycle() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    assert!(kv.del("k"));
    assert_eq!(kv.get("k"), None);
    // Re-setting after delete works and yields a fresh value.
    kv.set("k".to_string(), Value::Int(2), None);
    assert_eq!(kv.get("k"), Some(Value::Int(2)));
}

#[test]
fn incr_then_overwrite_with_set_then_incr_again() {
    let kv = KvStore::new();
    kv.incr("c"); // 1
    kv.set("c".to_string(), Value::Int(100), None); // overwrite
    assert_eq!(kv.incr("c"), Some(Value::Int(101)));
}

#[test]
fn list_and_scalar_keys_coexist_independently() {
    let kv = KvStore::new();
    kv.set("scalar".to_string(), Value::Int(1), None);
    kv.lpush("list", s("a"));
    kv.lpush("list", s("b"));
    assert_eq!(kv.get("scalar"), Some(Value::Int(1)));
    assert_eq!(kv.lrange("list", 0, 10), Some(vec![s("b"), s("a")]));
    // get on a list key returns the whole list value.
    assert_eq!(kv.get("list"), Some(Value::List(vec![s("b"), s("a")])));
}

#[test]
fn many_keys_are_all_retrievable() {
    let kv = KvStore::new();
    for i in 0..1000 {
        kv.set(format!("k{i}"), Value::Int(i), None);
    }
    assert_eq!(kv.snapshot().len(), 1000);
    assert_eq!(kv.get("k0"), Some(Value::Int(0)));
    assert_eq!(kv.get("k999"), Some(Value::Int(999)));
    assert_eq!(kv.get("k1000"), None);
}

// ===========================================================================
// TTL — boundary semantics: lazy `get` uses STRICT `>` (now > exp)
// ===========================================================================
//
// The source distinguishes two clocks:
//   * lazy `get`:  evicts only when `now_secs() > exp`  (strictly past)
//   * background:  evicts when      `exp <= now`        (at-or-past)
// These tests pin the lazy-read boundary, which is the deterministic one.

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn get_entry_expiring_exactly_now_is_still_present() {
    // expires_at == now: `now > exp` is false, so the entry is NOT evicted by a
    // lazy read. (Strict-greater boundary.)
    let kv = KvStore::new();
    let now = now_secs();
    let mut entries = HashMap::new();
    entries.insert(
        "edge".to_string(),
        KvEntry {
            value: Value::Int(1),
            expires_at: Some(now),
        },
    );
    kv.restore(entries);
    assert_eq!(kv.get("edge"), Some(Value::Int(1)));
    // And it remains physically present (not lazily removed).
    assert!(kv.snapshot().contains_key("edge"));
}

#[test]
fn get_entry_expiring_one_second_ago_is_absent() {
    // expires_at == now - 1: `now > exp` is true, so the entry is evicted.
    let kv = KvStore::new();
    let past = now_secs().saturating_sub(1);
    let mut entries = HashMap::new();
    entries.insert(
        "edge".to_string(),
        KvEntry {
            value: Value::Int(1),
            expires_at: Some(past),
        },
    );
    kv.restore(entries);
    assert_eq!(kv.get("edge"), None);
}

#[test]
fn set_with_zero_ttl_records_expiry_equal_to_now() {
    // ttl_secs Some(0) => expires_at = now + 0 = now. By the strict-> boundary
    // the value is still readable in the same second it was written.
    let kv = KvStore::new();
    let before = now_secs();
    kv.set("z".to_string(), Value::Int(9), Some(0));
    let snap = kv.snapshot();
    let exp = snap
        .get("z")
        .and_then(|e| e.expires_at)
        .expect("zero-ttl still records an expires_at");
    let after = now_secs();
    assert!(exp >= before && exp <= after, "exp={exp} before={before} after={after}");
    // Readable now (now > exp is false while now == exp).
    assert_eq!(kv.get("z"), Some(Value::Int(9)));
}

// ===========================================================================
// TTL — mutating ops do NOT consult expiry, and PRESERVE existing expires_at
// ===========================================================================

#[test]
fn incr_preserves_existing_ttl() {
    // incr mutates entry.value via get_mut and never touches expires_at, so a
    // counter with a TTL keeps that TTL after incrementing.
    let kv = KvStore::new();
    let future = now_secs() + 10_000;
    let mut entries = HashMap::new();
    entries.insert(
        "c".to_string(),
        KvEntry {
            value: Value::Int(5),
            expires_at: Some(future),
        },
    );
    kv.restore(entries);
    assert_eq!(kv.incr("c"), Some(Value::Int(6)));
    let snap = kv.snapshot();
    assert_eq!(snap.get("c").and_then(|e| e.expires_at), Some(future));
}

#[test]
fn incr_does_not_check_expiry_resurrects_expired_int() {
    // incr uses get_mut, which does NOT lazily expire. An expired int is still
    // present as a physical row, so incr increments it rather than auto-creating
    // a fresh 1. (Contrast with `get`, which would have evicted it.)
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "c".to_string(),
        KvEntry {
            value: Value::Int(41),
            expires_at: Some(1), // long past
        },
    );
    kv.restore(entries);
    assert_eq!(kv.incr("c"), Some(Value::Int(42)));
}

#[test]
fn lpush_preserves_existing_ttl_on_existing_list() {
    // or_insert_with only fires when the key is absent; pushing onto an existing
    // list mutates items in place and leaves expires_at intact.
    let kv = KvStore::new();
    let future = now_secs() + 10_000;
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![Value::Int(1)]),
            expires_at: Some(future),
        },
    );
    kv.restore(entries);
    kv.lpush("l", Value::Int(2));
    let snap = kv.snapshot();
    assert_eq!(snap.get("l").and_then(|e| e.expires_at), Some(future));
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn lpush_replacing_non_list_keeps_old_ttl() {
    // When the existing value is a non-list, lpush rewrites entry.value to a
    // one-element list but does NOT reset expires_at. The old TTL survives.
    let kv = KvStore::new();
    let future = now_secs() + 10_000;
    let mut entries = HashMap::new();
    entries.insert(
        "k".to_string(),
        KvEntry {
            value: Value::Int(99),
            expires_at: Some(future),
        },
    );
    kv.restore(entries);
    kv.lpush("k", Value::Int(7));
    let snap = kv.snapshot();
    assert_eq!(snap.get("k").and_then(|e| e.expires_at), Some(future));
    assert_eq!(kv.lrange("k", 0, 100), Some(vec![Value::Int(7)]));
}

#[test]
fn lpush_onto_expired_list_does_not_auto_evict() {
    // lpush does not consult expiry; an expired-but-present list is pushed onto
    // as-is rather than recreated empty.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![Value::Int(1)]),
            expires_at: Some(1), // past
        },
    );
    kv.restore(entries);
    kv.lpush("l", Value::Int(2));
    // The pre-existing element is retained because no eviction happened.
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn lrange_does_not_check_expiry() {
    // lrange reads via data.get (not self.get), so an expired list is still
    // returned rather than read as absent.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![Value::Int(1), Value::Int(2)]),
            expires_at: Some(1), // past
        },
    );
    kv.restore(entries);
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(1), Value::Int(2)])
    );
}

#[test]
fn ltrim_does_not_check_expiry() {
    // ltrim uses get_mut and also ignores expiry: it trims an expired list.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            expires_at: Some(1), // past
        },
    );
    kv.restore(entries);
    assert!(kv.ltrim("l", 0, 1));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![Value::Int(1)]));
}

// ===========================================================================
// LRANGE / LTRIM — additional non-inverted boundary cases
// ===========================================================================

#[test]
fn lrange_zero_zero_on_empty_list_is_empty() {
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![]),
            expires_at: None,
        },
    );
    kv.restore(entries);
    assert_eq!(kv.lrange("l", 0, 0), Some(vec![]));
}

#[test]
fn lrange_both_bounds_beyond_len_is_empty() {
    // start and stop both clamp to len => s == e == len => empty.
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    assert_eq!(kv.lrange("l", 100, 200), Some(vec![]));
}

#[test]
fn lrange_single_index_window_returns_one_element() {
    // [3,2,1]; window [2..3) -> the tail element only.
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    kv.lpush("l", Value::Int(3));
    assert_eq!(kv.lrange("l", 2, 3), Some(vec![Value::Int(1)]));
}

#[test]
fn ltrim_keeps_whole_list_with_full_window() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2)); // [2,1]
    assert!(kv.ltrim("l", 0, 2));
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn ltrim_drops_head_keeping_tail_window() {
    // [3,2,1]; trim to [1..3) -> [2,1].
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    kv.lpush("l", Value::Int(3));
    assert!(kv.ltrim("l", 1, 3));
    assert_eq!(
        kv.lrange("l", 0, 100),
        Some(vec![Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn ltrim_on_empty_list_returns_true_and_stays_empty() {
    // An explicitly empty list is a List, so ltrim returns true; s == e == 0.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "l".to_string(),
        KvEntry {
            value: Value::List(vec![]),
            expires_at: None,
        },
    );
    kv.restore(entries);
    assert!(kv.ltrim("l", 0, 0));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![]));
}

#[test]
fn ltrim_null_value_returns_false() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Null, None);
    assert!(!kv.ltrim("k", 0, 1));
    assert_eq!(kv.get("k"), Some(Value::Null));
}

#[test]
fn lrange_on_null_value_is_none() {
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Null, None);
    assert_eq!(kv.lrange("k", 0, 10), None);
}

#[test]
fn lpush_then_get_returns_list_variant() {
    // Round-trip: a list built by lpush is observable through get as a List.
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    assert_eq!(kv.get("l"), Some(Value::List(vec![Value::Int(2), Value::Int(1)])));
}

// ===========================================================================
// FLOAT — bit-pattern round-trips through set/get (Value::Float is bits)
// ===========================================================================

#[test]
fn set_get_float_zero_and_negative_zero_round_trip() {
    let kv = KvStore::new();
    kv.set("pos".to_string(), Value::from_f64(0.0), None);
    kv.set("neg".to_string(), Value::from_f64(-0.0), None);
    assert_eq!(kv.get("pos").and_then(|v| v.to_f64()), Some(0.0));
    assert_eq!(kv.get("neg").and_then(|v| v.to_f64()), Some(-0.0));
    // +0.0 and -0.0 have distinct bit patterns => distinct stored Values.
    assert_ne!(kv.get("pos"), kv.get("neg"));
}

#[test]
fn set_get_float_infinities_round_trip() {
    let kv = KvStore::new();
    kv.set("inf".to_string(), Value::from_f64(f64::INFINITY), None);
    kv.set("ninf".to_string(), Value::from_f64(f64::NEG_INFINITY), None);
    assert_eq!(kv.get("inf").and_then(|v| v.to_f64()), Some(f64::INFINITY));
    assert_eq!(
        kv.get("ninf").and_then(|v| v.to_f64()),
        Some(f64::NEG_INFINITY)
    );
}

#[test]
fn set_get_float_nan_round_trips_by_bits() {
    // NaN != NaN by value, but Value::Float compares by bit pattern, so the
    // stored Value equals itself and the recovered f64 is still NaN.
    let kv = KvStore::new();
    let nan = Value::from_f64(f64::NAN);
    kv.set("nan".to_string(), nan.clone(), None);
    assert_eq!(kv.get("nan"), Some(nan));
    assert!(kv.get("nan").and_then(|v| v.to_f64()).unwrap().is_nan());
}

#[test]
fn set_get_float_extremes_round_trip() {
    let kv = KvStore::new();
    kv.set("max".to_string(), Value::from_f64(f64::MAX), None);
    kv.set("min_pos".to_string(), Value::from_f64(f64::MIN_POSITIVE), None);
    assert_eq!(kv.get("max").and_then(|v| v.to_f64()), Some(f64::MAX));
    assert_eq!(
        kv.get("min_pos").and_then(|v| v.to_f64()),
        Some(f64::MIN_POSITIVE)
    );
}

// ===========================================================================
// NESTED / DEEP VALUE STRUCTURES — round-trips
// ===========================================================================

#[test]
fn set_get_list_of_maps_round_trips() {
    let kv = KvStore::new();
    let mut m1 = HashMap::new();
    m1.insert("id".to_string(), Value::Int(1));
    let mut m2 = HashMap::new();
    m2.insert("id".to_string(), Value::Int(2));
    let v = Value::List(vec![Value::Map(m1), Value::Map(m2)]);
    kv.set("rows".to_string(), v.clone(), None);
    assert_eq!(kv.get("rows"), Some(v));
}

#[test]
fn set_get_map_of_lists_round_trips() {
    let kv = KvStore::new();
    let mut m = HashMap::new();
    m.insert("a".to_string(), Value::List(vec![Value::Int(1), Value::Int(2)]));
    m.insert("b".to_string(), Value::List(vec![s("x")]));
    let v = Value::Map(m);
    kv.set("doc".to_string(), v.clone(), None);
    assert_eq!(kv.get("doc"), Some(v));
}

#[test]
fn set_get_deeply_nested_value_round_trips() {
    let kv = KvStore::new();
    let mut inner = HashMap::new();
    inner.insert("deep".to_string(), Value::List(vec![Value::Null, Value::Bool(false)]));
    let v = Value::List(vec![Value::Map(inner), Value::List(vec![Value::List(vec![
        Value::Int(7),
    ])])]);
    kv.set("nested".to_string(), v.clone(), None);
    assert_eq!(kv.get("nested"), Some(v));
}

#[test]
fn set_get_empty_collections_round_trip() {
    let kv = KvStore::new();
    kv.set("el".to_string(), Value::List(vec![]), None);
    kv.set("em".to_string(), Value::Map(HashMap::new()), None);
    assert_eq!(kv.get("el"), Some(Value::List(vec![])));
    assert_eq!(kv.get("em"), Some(Value::Map(HashMap::new())));
}

// ===========================================================================
// PUB/SUB — receiver-drop, message types, empty channel name
// ===========================================================================

#[test]
fn publish_after_only_subscriber_dropped_returns_zero() {
    // The broadcast sender stays in the pubsub map, but with no live receivers
    // `send` errors and publish maps that to 0.
    let kv = KvStore::new();
    let rx = kv.subscribe("c");
    drop(rx);
    assert_eq!(kv.publish("c", Value::Int(1)), 0);
}

#[test]
fn publish_counts_only_live_receivers_after_partial_drop() {
    let kv = KvStore::new();
    let rx1 = kv.subscribe("c");
    let mut rx2 = kv.subscribe("c");
    drop(rx1);
    // One receiver remains live.
    assert_eq!(kv.publish("c", Value::Int(5)), 1);
    assert_eq!(rx2.try_recv().ok(), Some(Value::Int(5)));
}

#[test]
fn resubscribe_after_drop_reuses_existing_channel() {
    // Dropping all receivers does not remove the sender from the map; a later
    // subscribe attaches a new receiver to the same channel.
    let kv = KvStore::new();
    let rx = kv.subscribe("c");
    drop(rx);
    let mut rx2 = kv.subscribe("c");
    assert_eq!(kv.publish("c", Value::Int(2)), 1);
    assert_eq!(rx2.try_recv().ok(), Some(Value::Int(2)));
}

#[test]
fn publish_delivers_complex_value_payloads() {
    let kv = KvStore::new();
    let mut rx = kv.subscribe("c");
    let payload = Value::List(vec![s("a"), Value::Int(1), Value::Null]);
    assert_eq!(kv.publish("c", payload.clone()), 1);
    assert_eq!(rx.try_recv().ok(), Some(payload));
}

#[test]
fn publish_to_empty_channel_name_works() {
    let kv = KvStore::new();
    let mut rx = kv.subscribe("");
    assert_eq!(kv.publish("", Value::Int(1)), 1);
    assert_eq!(rx.try_recv().ok(), Some(Value::Int(1)));
}

#[test]
fn try_recv_on_fresh_subscriber_with_no_publish_is_err() {
    // Empty broadcast buffer => try_recv returns Err (not a value).
    let kv = KvStore::new();
    let mut rx = kv.subscribe("c");
    assert!(rx.try_recv().is_err());
}

// ===========================================================================
// SNAPSHOT / RESTORE — TTL fidelity, expired entries survive snapshot
// ===========================================================================

#[test]
fn snapshot_captures_exact_expires_at_value() {
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "k".to_string(),
        KvEntry {
            value: Value::Int(1),
            expires_at: Some(123_456_789),
        },
    );
    kv.restore(entries);
    let snap = kv.snapshot();
    assert_eq!(snap.get("k").and_then(|e| e.expires_at), Some(123_456_789));
}

#[test]
fn snapshot_includes_expired_but_unread_entries() {
    // snapshot iterates the raw map without expiring; an expired row that has
    // not been lazily evicted is still present in the snapshot.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "stale".to_string(),
        KvEntry {
            value: Value::Int(1),
            expires_at: Some(1),
        },
    );
    kv.restore(entries);
    assert!(kv.snapshot().contains_key("stale"));
}

#[test]
fn restore_round_trip_preserves_past_and_future_ttls() {
    // A snapshot mixing a past-TTL and a future-TTL entry restores both rows
    // verbatim; only a subsequent `get` applies lazy expiry.
    let kv = KvStore::new();
    let future = now_secs() + 10_000;
    let mut entries = HashMap::new();
    entries.insert(
        "past".to_string(),
        KvEntry { value: Value::Int(1), expires_at: Some(1) },
    );
    entries.insert(
        "future".to_string(),
        KvEntry { value: Value::Int(2), expires_at: Some(future) },
    );
    kv.restore(entries);
    // Both rows exist physically right after restore.
    let snap = kv.snapshot();
    assert_eq!(snap.get("past").and_then(|e| e.expires_at), Some(1));
    assert_eq!(snap.get("future").and_then(|e| e.expires_at), Some(future));
    // Reading applies lazy expiry: past is gone, future is alive.
    assert_eq!(kv.get("past"), None);
    assert_eq!(kv.get("future"), Some(Value::Int(2)));
}

#[test]
fn restore_overwrites_only_data_not_pubsub_channels() {
    // restore clears/replaces the data map; it does not touch pubsub, so an
    // existing subscription keeps working across a restore.
    let kv = KvStore::new();
    let mut rx = kv.subscribe("c");
    kv.restore(HashMap::new());
    assert_eq!(kv.publish("c", Value::Int(1)), 1);
    assert_eq!(rx.try_recv().ok(), Some(Value::Int(1)));
}

#[test]
fn snapshot_returns_independent_copy_not_live_view() {
    // The snapshot is a clone; mutating the store afterward does not change it.
    let kv = KvStore::new();
    kv.set("k".to_string(), Value::Int(1), None);
    let snap = kv.snapshot();
    kv.set("k".to_string(), Value::Int(2), None);
    kv.del("k");
    assert_eq!(snap.get("k").map(|e| e.value.clone()), Some(Value::Int(1)));
}

// ===========================================================================
// MIXED / IDEMPOTENCY — additional sequences
// ===========================================================================

#[test]
fn repeated_set_same_value_is_idempotent_on_read() {
    let kv = KvStore::new();
    for _ in 0..5 {
        kv.set("k".to_string(), Value::Int(7), None);
    }
    assert_eq!(kv.get("k"), Some(Value::Int(7)));
    assert_eq!(kv.snapshot().len(), 1);
}

#[test]
fn incr_after_lazy_expiry_auto_creates_fresh_counter() {
    // get evicts the expired int; the subsequent incr finds no row and creates 1.
    let kv = KvStore::new();
    let mut entries = HashMap::new();
    entries.insert(
        "c".to_string(),
        KvEntry { value: Value::Int(50), expires_at: Some(1) },
    );
    kv.restore(entries);
    assert_eq!(kv.get("c"), None); // triggers eviction
    assert_eq!(kv.incr("c"), Some(Value::Int(1)));
}

#[test]
fn del_then_lpush_creates_fresh_list() {
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.lpush("l", Value::Int(2));
    assert!(kv.del("l"));
    kv.lpush("l", Value::Int(9));
    assert_eq!(kv.lrange("l", 0, 100), Some(vec![Value::Int(9)]));
}

#[test]
fn set_clears_list_semantics_so_lrange_reads_none() {
    // Overwriting a list key with a scalar makes lrange report a non-list (None).
    let kv = KvStore::new();
    kv.lpush("l", Value::Int(1));
    kv.set("l".to_string(), Value::Int(5), None);
    assert_eq!(kv.lrange("l", 0, 10), None);
    assert_eq!(kv.get("l"), Some(Value::Int(5)));
}

#[test]
fn snapshot_round_trip_preserves_pubsub_independence_of_new_store() {
    // Data crosses via snapshot/restore, but pubsub state does NOT — the
    // restored store starts with no channels.
    let kv = KvStore::new();
    let _rx = kv.subscribe("c");
    kv.set("k".to_string(), Value::Int(1), None);

    let other = KvStore::new();
    other.restore(kv.snapshot());
    assert_eq!(other.get("k"), Some(Value::Int(1)));
    // No subscriber was carried over.
    assert_eq!(other.publish("c", Value::Int(1)), 0);
}
