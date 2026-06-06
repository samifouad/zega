use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::interval;
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

    pub fn start_eviction_task(&self) {
        #[cfg(target_arch = "wasm32")]
        {
            // Expired values are removed lazily on access in wasm builds.
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let data = self.data.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                rt.block_on(async move {
                    let mut ticker = interval(Duration::from_secs(1));
                    loop {
                        ticker.tick().await;
                        let now = Self::now_secs();
                        let to_remove: Vec<String> = data
                            .iter()
                            .filter(|entry| {
                                if let Some(exp) = entry.value().expires_at {
                                    exp <= now
                                } else {
                                    false
                                }
                            })
                            .map(|entry| entry.key().clone())
                            .collect();
                        for key in to_remove {
                            data.remove(&key);
                        }
                    }
                });
            });
        }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        if let Some(entry) = self.data.get(key) {
            if let Some(exp) = entry.expires_at {
                if Self::now_secs() > exp {
                    drop(entry);
                    self.data.remove(key);
                    return None;
                }
            }
            Some(entry.value.clone())
        } else {
            None
        }
    }

    pub fn set(&self, key: String, value: Value, ttl_secs: Option<u64>) {
        let expires_at = ttl_secs.map(|secs| Self::now_secs() + secs);
        self.data.insert(key, KvEntry { value, expires_at });
    }

    pub fn del(&self, key: &str) -> bool {
        self.data.remove(key).is_some()
    }

    pub fn incr(&self, key: &str) -> Option<Value> {
        if let Some(mut entry) = self.data.get_mut(key) {
            match &entry.value {
                Value::Int(n) => {
                    let new_val = Value::Int(n + 1);
                    entry.value = new_val.clone();
                    Some(new_val)
                }
                _ => None,
            }
        } else {
            let val = Value::Int(1);
            self.data.insert(
                key.to_string(),
                KvEntry {
                    value: val.clone(),
                    expires_at: None,
                },
            );
            Some(val)
        }
    }

    pub fn lpush(&self, key: &str, value: Value) {
        let mut entry = self.data.entry(key.to_string()).or_insert_with(|| KvEntry {
            value: Value::List(vec![]),
            expires_at: None,
        });
        match &mut entry.value {
            Value::List(ref mut items) => items.insert(0, value),
            _ => {
                entry.value = Value::List(vec![value]);
            }
        }
    }

    pub fn lrange(&self, key: &str, start: usize, stop: usize) -> Option<Vec<Value>> {
        self.data.get(key).and_then(|entry| match &entry.value {
            Value::List(items) => {
                let len = items.len();
                let s = start.min(len);
                let e = stop.min(len);
                Some(items[s..e].to_vec())
            }
            _ => None,
        })
    }

    pub fn ltrim(&self, key: &str, start: usize, stop: usize) -> bool {
        if let Some(mut entry) = self.data.get_mut(key) {
            match &mut entry.value {
                Value::List(items) => {
                    let len = items.len();
                    let s = start.min(len);
                    let e = stop.min(len);
                    *items = items[s..e].to_vec();
                    true
                }
                _ => false,
            }
        } else {
            false
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
            .map(|e| (e.key().clone(), e.value().clone()))
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
}
