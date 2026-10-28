//! Client telemetry: a persisted queue flushed in batches to
//! POST /api/telemetry/client (port of ClientTelemetry.swift).

use crate::RT;
use crate::client::{CoreConfig, Inner};
use crate::store::{PlatformStore, keys};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

const MAX_QUEUED: usize = 300;
const BATCH: usize = 50;
const MAX_BATCHES: usize = 6;
const WEDGE: Duration = Duration::from_secs(120);

#[derive(Clone, Serialize, Deserialize)]
struct Event {
    name: String,
    at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    metadata: HashMap<String, String>,
}

struct State {
    queue: Vec<Event>,
    flushing_since: Option<Instant>,
}

pub struct Telemetry {
    client: String,
    app_version: String,
    os_version: String,
    device_model: String,
    pub launch_id: String,
    store: Arc<dyn PlatformStore>,
    state: Mutex<State>,
    owner: Mutex<Weak<Inner>>,
}

impl Telemetry {
    pub fn new(cfg: &CoreConfig, store: Arc<dyn PlatformStore>) -> Self {
        let queue = store.get(keys::TELEMETRY_QUEUE.into(), false).and_then(|s| serde_json::from_str::<Vec<Event>>(&s).ok()).unwrap_or_default();
        Telemetry {
            client: cfg.client.clone(),
            app_version: cfg.app_version.clone(),
            os_version: cfg.os_version.clone(),
            device_model: cfg.device_model.clone(),
            launch_id: uuid::Uuid::new_v4().to_string().to_uppercase(),
            store,
            state: Mutex::new(State { queue, flushing_since: None }),
            owner: Mutex::new(Weak::new()),
        }
    }

    /// Wires the owning client in and starts the periodic flush (5s, then every 60s).
    pub(crate) fn attach(&self, owner: Weak<Inner>) {
        *self.owner.lock().unwrap() = owner.clone();
        RT.spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                let Some(i) = owner.upgrade() else { break };
                i.telemetry.flush().await;
                drop(i);
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    pub fn log(&self, name: &str, value: Option<f64>, text: Option<String>, mut metadata: HashMap<String, String>) {
        metadata.insert("launch_id".into(), self.launch_id.clone());
        let at = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        let full = {
            let mut s = self.state.lock().unwrap();
            s.queue.push(Event { name: name.to_string(), at, value, text, metadata });
            let n = s.queue.len();
            if n > MAX_QUEUED {
                s.queue.drain(0..n - MAX_QUEUED);
            }
            self.persist(&s.queue);
            s.queue.len() >= BATCH
        };
        if full {
            let owner = self.owner.lock().unwrap().clone();
            RT.spawn(async move {
                if let Some(i) = owner.upgrade() {
                    i.telemetry.flush().await;
                }
            });
        }
    }

    fn persist(&self, q: &[Event]) {
        if let Ok(s) = serde_json::to_string(q) {
            self.store.set(keys::TELEMETRY_QUEUE.into(), Some(s), false);
        }
    }

    pub async fn flush(&self) {
        {
            let mut s = self.state.lock().unwrap();
            if let Some(t) = s.flushing_since {
                if t.elapsed() < WEDGE {
                    return;
                }
            }
            if s.queue.is_empty() {
                return;
            }
            s.flushing_since = Some(Instant::now());
        }
        let Some(owner) = self.owner.lock().unwrap().upgrade() else {
            self.state.lock().unwrap().flushing_since = None;
            return;
        };
        for _ in 0..MAX_BATCHES {
            let events: Vec<Event> = {
                let s = self.state.lock().unwrap();
                if s.queue.is_empty() {
                    break;
                }
                s.queue.iter().take(BATCH).cloned().collect()
            };
            let body = json!({
                "client": self.client, "app_version": self.app_version, "os_version": self.os_version,
                "device_model": self.device_model, "launch_id": self.launch_id, "events": events,
            });
            if owner.request("POST", "/api/telemetry/client", &[], Some(body)).await.is_err() {
                break;
            }
            let mut s = self.state.lock().unwrap();
            let n = events.len().min(s.queue.len());
            s.queue.drain(0..n);
            self.persist(&s.queue);
        }
        self.state.lock().unwrap().flushing_since = None;
    }
}
