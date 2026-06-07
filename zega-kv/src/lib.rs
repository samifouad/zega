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

    fn remove_if_expired(&self, key: &str) -> bool {
        let now = Self::now_secs();
        self.data
            .remove_if(key, |_, entry| Self::is_expired(entry, now))
            .is_some()
    }

    fn purge_expired(&self) {
        let now = Self::now_secs();
        self.data.retain(|_, entry| !Self::is_expired(entry, now));
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        let value = self.data.get(key).and_then(|entry| {
            (!Self::is_expired(&entry, Self::now_secs())).then(|| entry.value.clone())
        });
        if value.is_none() {
            self.remove_if_expired(key);
        }
        value
    }

    pub fn set(&self, key: String, value: Value, ttl_secs: Option<u64>) {
        let expires_at = ttl_secs.map(|secs| Self::now_secs() + secs);
        self.data.insert(key, KvEntry { value, expires_at });
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

    pub fn lrange(&self, key: &str, start: usize, stop: usize) -> Option<Vec<Value>> {
        let values = self.data.get(key).and_then(|entry| {
            if Self::is_expired(&entry, Self::now_secs()) {
                None
            } else {
                match &entry.value {
                    Value::List(items) => {
                        let len = items.len();
                        let s = start.min(len);
                        let e = stop.min(len);
                        Some(items[s..e].to_vec())
                    }
                    _ => None,
                }
            }
        });
        if values.is_none() {
            self.remove_if_expired(key);
        }
        values
    }

    pub fn ltrim(&self, key: &str, start: usize, stop: usize) -> bool {
        match self.data.entry(key.to_string()) {
            Entry::Occupied(entry) if Self::is_expired(entry.get(), Self::now_secs()) => {
                entry.remove();
                false
            }
            Entry::Occupied(mut entry) => match &mut entry.get_mut().value {
                Value::List(items) => {
                    let len = items.len();
                    let s = start.min(len);
                    let e = stop.min(len);
                    *items = items[s..e].to_vec();
                    true
                }
                _ => false,
            },
            Entry::Vacant(_) => false,
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
        let snapshot = self
            .data
            .iter()
            .filter_map(|entry| {
                (!Self::is_expired(entry.value(), Self::now_secs()))
                    .then(|| (entry.key().clone(), entry.value().clone()))
            })
            .collect();
        self.purge_expired();
        snapshot
    }

    /// Restore entries from a snapshot.
    pub fn restore(&self, entries: HashMap<String, KvEntry>) {
        self.data.clear();
        for (k, v) in entries {
            self.data.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kv_set_get_del() {
        let kv = KvStore::new();
        kv.set("name".to_string(), Value::String("Alice".to_string()), None);
        assert_eq!(kv.get("name"), Some(Value::String("Alice".to_string())));
        assert!(kv.del("name"));
        assert_eq!(kv.get("name"), None);
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
            kv.lrange("mylist", 0, 10),
            Some(vec![Value::Int(2), Value::Int(1)])
        );
        kv.ltrim("mylist", 0, 1);
        assert_eq!(kv.lrange("mylist", 0, 10), Some(vec![Value::Int(2)]));
    }

    #[test]
    fn test_expired_key_is_removed_lazily() {
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
}
