use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use zega_parser::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KvEntry {
    pub value: Value,
    /// Expiration timestamp in seconds since UNIX epoch.
    pub expires_at: Option<u64>,
}

pub struct KvStore {
    data: Arc<DashMap<String, KvEntry>>,
    pubsub: Arc<DashMap<String, broadcast::Sender<Value>>>,
}

impl Default for KvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KvStore {
    pub fn new() -> Self {
        KvStore {
            data: Arc::new(DashMap::new()),
            pubsub: Arc::new(DashMap::new()),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn is_expired(entry: &KvEntry, now: u64) -> bool {
        matches!(entry.expires_at, Some(expires_at) if expires_at <= now)
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.data.get(key).and_then(|entry| {
            (!Self::is_expired(&entry, Self::now_secs())).then(|| entry.value.clone())
        })
    }

    pub fn exists(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn ttl(&self, key: &str) -> Option<u64> {
        let now = Self::now_secs();
        self.data.get(key).and_then(|entry| {
            if Self::is_expired(&entry, now) {
                None
            } else {
                entry.expires_at.map(|expires_at| expires_at - now)
            }
        })
    }

    pub fn expire(&self, key: &str, ttl_secs: u64) -> bool {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                entry.remove();
                false
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().expires_at = Some(Self::now_secs() + ttl_secs);
                true
            }
            Entry::Vacant(_) => false,
        }
    }

    pub fn set(&self, key: String, value: Value, ttl_secs: Option<u64>) {
        let expires_at = ttl_secs.map(|secs| Self::now_secs() + secs);
        self.data.insert(key, KvEntry { value, expires_at });
    }

    pub fn set_nx(&self, key: String, value: Value, ttl_secs: Option<u64>) -> bool {
        let now = Self::now_secs();
        let expires_at = ttl_secs.map(|secs| now + secs);
        let new_entry = KvEntry { value, expires_at };
        match self.data.entry(key) {
            Entry::Occupied(mut entry) if Self::is_expired(entry.get(), now) => {
                entry.insert(new_entry);
                true
            }
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(new_entry);
                true
            }
        }
    }

    pub fn del(&self, key: &str) -> bool {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                entry.remove();
                false
            }
            Entry::Occupied(entry) => {
                entry.remove();
                true
            }
            Entry::Vacant(_) => false,
        }
    }

    pub fn incr(&self, key: &str) -> Option<Value> {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(mut entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                let val = Value::Int(1);
                entry.insert(KvEntry {
                    value: val.clone(),
                    expires_at: None,
                });
                Some(val)
            }
            Entry::Occupied(mut entry) => match &mut entry.get_mut().value {
                Value::Int(n) => {
                    *n += 1;
                    Some(Value::Int(*n))
                }
                _ => None,
            },
            Entry::Vacant(entry) => {
                let val = Value::Int(1);
                entry.insert(KvEntry {
                    value: val.clone(),
                    expires_at: None,
                });
                Some(val)
            }
        }
    }

    pub fn incr_with_ttl(&self, key: &str, ttl_secs: u64) -> Option<Value> {
        let now = Self::now_secs();
        let expires_at = Some(now + ttl_secs);
        match self.data.entry(key.to_string()) {
            Entry::Occupied(mut entry) if Self::is_expired(entry.get(), now) => {
                let val = Value::Int(1);
                entry.insert(KvEntry {
                    value: val.clone(),
                    expires_at,
                });
                Some(val)
            }
            Entry::Occupied(mut entry) => {
                let e = entry.get_mut();
                match &mut e.value {
                    Value::Int(n) => {
                        *n += 1;
                        e.expires_at = expires_at;
                        Some(Value::Int(*n))
                    }
                    _ => None,
                }
            }
            Entry::Vacant(entry) => {
                let val = Value::Int(1);
                entry.insert(KvEntry {
                    value: val.clone(),
                    expires_at,
                });
                Some(val)
            }
        }
    }

    pub fn scan(
        &self,
        cursor: usize,
        pattern: &str,
        count: usize,
    ) -> (usize, Vec<String>) {
        let now = Self::now_secs();
        let mut keys: Vec<String> = self
            .data
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                let kv_entry = entry.value();
                if Self::is_expired(kv_entry, now) {
                    return None;
                }
                if pattern.is_empty() || key.starts_with(pattern) {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect();
        keys.sort();
        let count = count.max(1);
        let start = cursor.min(keys.len());
        let end = (start + count).min(keys.len());
        let next_cursor = if end >= keys.len() { 0 } else { end };
        (next_cursor, keys[start..end].to_vec())
    }

    pub fn lpush(&self, key: &str, value: Value) {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(mut entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                entry.insert(KvEntry {
                    value: Value::List(vec![value]),
                    expires_at: None,
                });
            }
            Entry::Occupied(mut entry) => match &mut entry.get_mut().value {
                Value::List(items) => items.insert(0, value),
                current => *current = Value::List(vec![value]),
            },
            Entry::Vacant(entry) => {
                entry.insert(KvEntry {
                    value: Value::List(vec![value]),
                    expires_at: None,
                });
            }
        }
    }

    pub fn rpush(&self, key: &str, value: Value) {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(mut entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                entry.insert(KvEntry {
                    value: Value::List(vec![value]),
                    expires_at: None,
                });
            }
            Entry::Occupied(mut entry) => match &mut entry.get_mut().value {
                Value::List(items) => items.push(value),
                current => *current = Value::List(vec![value]),
            },
            Entry::Vacant(entry) => {
                entry.insert(KvEntry {
                    value: Value::List(vec![value]),
                    expires_at: None,
                });
            }
        }
    }

    pub fn lrange(
        &self,
        key: &str,
        start: usize,
        stop: usize,
    ) -> Result<Option<Vec<Value>>, String> {
        self.data
            .get(key)
            .map(|entry| {
                if Self::is_expired(&entry, Self::now_secs()) {
                    Ok(None)
                } else {
                    match &entry.value {
                        Value::List(items) => {
                            let (start, stop) = checked_range(items.len(), start, stop)?;
                            Ok(Some(items[start..stop].to_vec()))
                        }
                        _ => Ok(None),
                    }
                }
            })
            .unwrap_or(Ok(None))
    }

    pub fn ltrim(&self, key: &str, start: usize, stop: usize) -> Result<bool, String> {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                entry.remove();
                Ok(false)
            }
            Entry::Occupied(mut entry) => match &mut entry.get_mut().value {
                Value::List(items) => {
                    let (start, stop) = checked_range(items.len(), start, stop)?;
                    *items = items[start..stop].to_vec();
                    Ok(true)
                }
                _ => Ok(false),
            },
            Entry::Vacant(_) => Ok(false),
        }
    }

    pub fn publish(&self, channel: &str, message: Value) -> usize {
        if let Some(sender) = self.pubsub.get(channel) {
            sender.send(message).unwrap_or(0)
        } else {
            0
        }
    }

    pub fn subscribe(&self, channel: &str) -> broadcast::Receiver<Value> {
        let sender = self
            .pubsub
            .entry(channel.to_string())
            .or_insert_with(|| broadcast::channel(1024).0)
            .clone();
        sender.subscribe()
    }

    /// Return a clone of all entries for snapshotting.
    pub fn snapshot(&self) -> HashMap<String, KvEntry> {
        self.data
            .iter()
            .filter_map(|entry| {
                (!Self::is_expired(entry.value(), Self::now_secs()))
                    .then(|| (entry.key().clone(), entry.value().clone()))
            })
            .collect()
    }

    /// Restore entries from a snapshot.
    pub fn restore(&self, entries: HashMap<String, KvEntry>) {
        self.data.clear();
        for (k, v) in entries {
            self.data.insert(k, v);
        }
    }
}

fn checked_range(len: usize, start: usize, stop: usize) -> Result<(usize, usize), String> {
    let start = start.min(len);
    let stop = stop.min(len);
    if start > stop {
        return Err(format!(
            "invalid list range: start ({start}) exceeds stop ({stop})"
        ));
    }
    Ok((start, stop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn test_kv_set_get_del() {
        let kv = KvStore::new();
        kv.set("name".to_string(), Value::String("Alice".to_string()), None);
        assert_eq!(kv.get("name"), Some(Value::String("Alice".to_string())));
        assert!(kv.del("name"));
        assert_eq!(kv.get("name"), None);
    }

    #[test]
    fn concurrent_set_nx_has_exactly_one_winner() {
        const CLAIMANTS: usize = 32;
        let kv = Arc::new(KvStore::new());
        let barrier = Arc::new(Barrier::new(CLAIMANTS));
        let claimants: Vec<_> = (0..CLAIMANTS)
            .map(|claimant| {
                let kv = Arc::clone(&kv);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    kv.set_nx("claim".to_string(), Value::Int(claimant as i64), None)
                })
            })
            .collect();

        let winners = claimants
            .into_iter()
            .map(|claimant| claimant.join().unwrap())
            .filter(|won| *won)
            .count();

        assert_eq!(winners, 1);
        assert!(kv.get("claim").is_some());
    }

    #[test]
    fn test_kv_incr() {
        let kv = KvStore::new();
        assert_eq!(kv.incr("counter"), Some(Value::Int(1)));
        assert_eq!(kv.incr("counter"), Some(Value::Int(2)));
        assert_eq!(kv.incr("counter"), Some(Value::Int(3)));
    }

    #[test]
    fn test_kv_list() {
        let kv = KvStore::new();
        kv.lpush("mylist", Value::Int(1));
        kv.lpush("mylist", Value::Int(2));
        assert_eq!(
            kv.lrange("mylist", 0, 10).unwrap(),
            Some(vec![Value::Int(2), Value::Int(1)])
        );
        kv.ltrim("mylist", 0, 1).unwrap();
        assert_eq!(
            kv.lrange("mylist", 0, 10).unwrap(),
            Some(vec![Value::Int(2)])
        );
    }

    #[test]
    fn write_paths_remove_expired_keys() {
        let kv = KvStore::new();
        kv.set(
            "session".to_string(),
            Value::String("abc".to_string()),
            Some(0),
        );

        assert_eq!(kv.get("session"), None);
        assert!(!kv.del("session"));
        assert!(kv.snapshot().is_empty());
    }

    #[test]
    fn malformed_list_ranges_return_errors_without_mutating() {
        let kv = KvStore::new();
        kv.lpush("list", Value::Int(1));
        kv.lpush("list", Value::Int(2));

        for (start, stop) in [(1, 0), (usize::MAX, 0), (usize::MAX, 1)] {
            assert!(kv.lrange("list", start, stop).is_err());
            assert!(kv.ltrim("list", start, stop).is_err());
        }
        assert_eq!(
            kv.lrange("list", 0, usize::MAX).unwrap(),
            Some(vec![Value::Int(2), Value::Int(1)])
        );
        assert_eq!(
            kv.lrange("list", usize::MAX, usize::MAX).unwrap(),
            Some(vec![])
        );
    }

    #[test]
    fn reads_do_not_remove_expired_entries() {
        let kv = KvStore::new();
        kv.set("value".to_string(), Value::Int(1), Some(0));
        kv.set(
            "list".to_string(),
            Value::List(vec![Value::Int(1)]),
            Some(0),
        );

        assert_eq!(kv.get("value"), None);
        assert!(!kv.exists("value"));
        assert_eq!(kv.ttl("value"), None);
        assert_eq!(kv.lrange("list", 0, 1).unwrap(), None);
        assert!(kv.snapshot().is_empty());
        assert!(kv.data.contains_key("value"));
        assert!(kv.data.contains_key("list"));
    }

    #[test]
    fn test_kv_rpush() {
        let kv = KvStore::new();
        kv.rpush("mylist", Value::Int(1));
        kv.rpush("mylist", Value::Int(2));
        assert_eq!(
            kv.lrange("mylist", 0, 10).unwrap(),
            Some(vec![Value::Int(1), Value::Int(2)])
        );
    }

    #[test]
    fn test_kv_incr_with_ttl() {
        let kv = KvStore::new();
        assert_eq!(kv.incr_with_ttl("counter", 10), Some(Value::Int(1)));
        assert_eq!(kv.incr_with_ttl("counter", 10), Some(Value::Int(2)));
        assert!(kv.ttl("counter").unwrap() <= 10);
        assert!(kv.ttl("counter").unwrap() > 0);
    }

    #[test]
    fn test_kv_scan() {
        let kv = KvStore::new();
        kv.set("aaa".to_string(), Value::Int(1), None);
        kv.set("aab".to_string(), Value::Int(2), None);
        kv.set("abc".to_string(), Value::Int(3), None);
        kv.set("bbb".to_string(), Value::Int(4), None);

        let (cursor, keys) = kv.scan(0, "aa", 10);
        assert_eq!(cursor, 0);
        assert_eq!(keys, vec!["aaa", "aab"]);

        let (cursor, keys) = kv.scan(0, "", 2);
        assert_eq!(cursor, 2);
        assert_eq!(keys, vec!["aaa", "aab"]);

        let (cursor, keys) = kv.scan(cursor, "", 10);
        assert_eq!(cursor, 0);
        assert_eq!(keys, vec!["abc", "bbb"]);
    }

    #[test]
    fn test_kv_scan_expired_keys_are_excluded() {
        let kv = KvStore::new();
        kv.set("active".to_string(), Value::Int(1), None);
        kv.set("expired".to_string(), Value::Int(2), Some(0));

        let (cursor, keys) = kv.scan(0, "", 10);
        assert_eq!(cursor, 0);
        assert_eq!(keys, vec!["active"]);
    }

    #[test]
    fn concurrent_incr_with_ttl_has_exactly_one_first_value() {
        const CLAIMANTS: usize = 32;
        let kv = Arc::new(KvStore::new());
        let barrier = Arc::new(Barrier::new(CLAIMANTS));
        let claimants: Vec<_> = (0..CLAIMANTS)
            .map(|_| {
                let kv = Arc::clone(&kv);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    kv.incr_with_ttl("claim", 60)
                })
            })
            .collect();

        let results: Vec<_> = claimants
            .into_iter()
            .map(|claimant| claimant.join().unwrap())
            .collect();

        assert_eq!(results.iter().filter(|r| r.is_some()).count(), CLAIMANTS);
        let first_values: Vec<_> = results.iter().filter(|r| matches!(r, Some(Value::Int(1)))).collect();
        assert_eq!(first_values.len(), 1, "exactly one thread should observe the first value");
        assert_eq!(kv.get("claim"), Some(Value::Int(CLAIMANTS as i64)));
    }
}
