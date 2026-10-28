//! Small TTL caches shared across requests.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Ttl {
    m: Mutex<HashMap<String, (Instant, Value)>>,
}

impl Ttl {
    pub fn get(&self, k: &str, ttl: Duration) -> Option<Value> {
        let m = self.m.lock().unwrap();
        m.get(k).filter(|(t, _)| t.elapsed() < ttl).map(|(_, v)| v.clone())
    }
    pub fn put(&self, k: &str, v: Value) {
        let mut m = self.m.lock().unwrap();
        if m.len() > 4000 {
            m.clear();
        }
        m.insert(k.to_string(), (Instant::now(), v));
    }
    pub fn remove(&self, k: &str) {
        self.m.lock().unwrap().remove(k);
    }
}

#[derive(Default)]
pub struct Caches {
    pub discord: Ttl,
    pub dashboard: Ttl,
    pub misc: Ttl,
    pub build_lock: tokio::sync::Mutex<()>,
}
