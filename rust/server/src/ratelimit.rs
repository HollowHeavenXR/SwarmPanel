//! Sliding-window limiter (port of ratelimit.lua): `limit` hits per `window` seconds per key.

use crate::http::{Reply, R};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<String, Vec<i64>>>,
}

const MAX_BUCKETS: usize = 2000;

impl Limiter {
    pub fn allow(&self, key: &str, limit: usize, window: i64) -> bool {
        let now = crate::util::now();
        let mut b = self.buckets.lock().unwrap();
        let hits = b.entry(key.to_string()).or_default();
        hits.retain(|t| now - t < window);
        if hits.len() >= limit {
            return false;
        }
        hits.push(now);
        if b.len() > MAX_BUCKETS {
            b.retain(|_, v| v.iter().any(|t| now - t < window));
            if b.len() > MAX_BUCKETS {
                let mut e: Vec<(String, i64)> = b.iter().map(|(k, v)| (k.clone(), v.iter().copied().max().unwrap_or(0))).collect();
                e.sort_by_key(|x| x.1);
                for (k, _) in e.iter().take(e.len() / 2) {
                    b.remove(k);
                }
            }
        }
        true
    }

    /// 429 "Too many authentication attempts" (the Lua's ratelimit.check).
    pub fn check(&self, key: &str, limit: usize, window: i64) -> R<()> {
        if self.allow(key, limit, window) { Ok(()) } else { Err(Reply::err(429, "Too many authentication attempts. Try again later.")) }
    }
}
