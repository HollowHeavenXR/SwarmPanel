//! Live updates over /ws (port of SwarmLiveSocket.swift): auth first frame,
//! watch/unwatch per key, typed snapshots to listeners, ping/pong,
//! exponential reconnect, and a stale-connection watchdog.

use crate::RT;
use crate::client::Inner;
use crate::lenient::Lenient;
use crate::models::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(uniffi::Enum, Clone, Debug)]
pub enum LiveSnapshot {
    Dashboard { value: DashboardResponse },
    Notifications { value: NotificationsSnapshot },
    Friends { value: FriendsSnapshot },
    CommunityCounts { value: CommunityCounts },
    Threads { value: MessageThreadsResponse },
    ThreadMessages { value: MessagesResponse },
    ControlState { value: ControlStateResponse },
    Account { value: AccountSnapshot },
    Other { json: String },
}

#[uniffi::export(with_foreign)]
pub trait LiveListener: Send + Sync {
    fn on_snapshot(&self, key: String, snapshot: LiveSnapshot);
    fn on_error(&self, key: String, message: String);
}

#[uniffi::export(with_foreign)]
pub trait LiveConnectionObserver: Send + Sync {
    fn connection_changed(&self, connected: bool);
}

struct Watch {
    params: Option<HashMap<String, String>>,
    listener: Arc<dyn LiveListener>,
}

#[derive(Default)]
struct State {
    watches: HashMap<String, Watch>,
    tx: Option<mpsc::UnboundedSender<String>>,
    connected: bool,
    want: bool,
    task: Option<tokio::task::JoinHandle<()>>,
    observers: Vec<Arc<dyn LiveConnectionObserver>>,
    connected_at: Option<Instant>,
    disconnects: u32,
}

#[derive(uniffi::Object)]
pub struct LiveSocket {
    owner: Weak<Inner>,
    state: Mutex<State>,
    me: Mutex<Weak<LiveSocket>>,
}

fn watch_frame(key: &str, params: &Option<HashMap<String, String>>) -> String {
    match params {
        Some(p) => json!({ "type": "watch", "key": key, "params": p }).to_string(),
        None => json!({ "type": "watch", "key": key }).to_string(),
    }
}

fn decode(key: &str, data: &Value) -> LiveSnapshot {
    fn d<T: Lenient + Default>(v: &Value) -> T {
        T::from_json(v).unwrap_or_default()
    }
    match key {
        "dashboard" => LiveSnapshot::Dashboard { value: d(data) },
        "notifications" => LiveSnapshot::Notifications { value: d(data) },
        "friends" => LiveSnapshot::Friends { value: d(data) },
        "community_counts" => LiveSnapshot::CommunityCounts { value: d(data) },
        "threads" => LiveSnapshot::Threads { value: d(data) },
        "thread_messages" => LiveSnapshot::ThreadMessages { value: d(data) },
        "control_state" => LiveSnapshot::ControlState { value: d(data) },
        "account" => LiveSnapshot::Account { value: d(data) },
        _ => LiveSnapshot::Other { json: data.to_string() },
    }
}

impl LiveSocket {
    pub(crate) fn new(owner: Weak<Inner>) -> Arc<Self> {
        let s = Arc::new(LiveSocket { owner, state: Mutex::default(), me: Mutex::default() });
        *s.me.lock().unwrap() = Arc::downgrade(&s);
        s
    }

    fn send(&self, text: String) {
        if let Some(tx) = &self.state.lock().unwrap().tx {
            let _ = tx.send(text);
        }
    }

    fn set_connected(&self, connected: bool) {
        let (observers, changed) = {
            let mut s = self.state.lock().unwrap();
            let changed = s.connected != connected;
            s.connected = connected;
            (s.observers.clone(), changed)
        };
        if !changed {
            return;
        }
        if let Some(i) = self.owner.upgrade() {
            let mut s = self.state.lock().unwrap();
            if connected {
                s.connected_at = Some(Instant::now());
                i.telemetry.log("socket_connected", None, None, HashMap::from([("disconnects_this_launch".into(), s.disconnects.to_string())]));
            } else {
                s.disconnects += 1;
                let up = s.connected_at.take().map(|t| t.elapsed().as_secs_f64().round());
                i.telemetry.log("socket_disconnected", up, None, HashMap::from([("disconnects_this_launch".into(), s.disconnects.to_string())]));
            }
        }
        for o in observers {
            o.connection_changed(connected);
        }
    }

    fn dispatch(&self, text: &str) -> Option<String> {
        let v: Value = serde_json::from_str(text).ok()?;
        match v.get("type").and_then(Value::as_str)? {
            "ping" => return Some(json!({ "type": "pong" }).to_string()),
            "snapshot" => {
                let key = v.get("key").and_then(Value::as_str)?.to_string();
                let listener = self.state.lock().unwrap().watches.get(&key).map(|w| w.listener.clone())?;
                listener.on_snapshot(key.clone(), decode(&key, v.get("data").unwrap_or(&Value::Null)));
            }
            "snapshot_error" => {
                let key = v.get("key").and_then(Value::as_str)?.to_string();
                let listener = self.state.lock().unwrap().watches.get(&key).map(|w| w.listener.clone())?;
                listener.on_error(key, v.get("error").and_then(Value::as_str).unwrap_or("Unknown error").to_string());
            }
            _ => {}
        }
        None
    }

    /// One connection lifetime. Returns when the socket drops; Err(true) = auth rejected (stop).
    async fn session(self: &Arc<Self>, url: &str, token: &str) -> Result<(), bool> {
        let (ws, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(url)).await.map_err(|_| false)?.map_err(|_| false)?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        sink.send(Message::Text(json!({ "type": "auth", "token": token }).to_string().into())).await.map_err(|_| false)?;
        {
            let mut s = self.state.lock().unwrap();
            for (k, w) in &s.watches {
                let _ = tx.send(watch_frame(k, &w.params));
            }
            s.tx = Some(tx.clone());
        }
        self.set_connected(true);
        let result = loop {
            tokio::select! {
                out = rx.recv() => {
                    let Some(out) = out else { break Ok(()) };
                    if sink.send(Message::Text(out.into())).await.is_err() { break Ok(()) }
                }
                msg = tokio::time::timeout(Duration::from_secs(120), stream.next()) => {
                    match msg {
                        Err(_) => break Ok(()), // stale: no frame (not even a ping) in 120s
                        Ok(None) | Ok(Some(Err(_))) => break Ok(()),
                        Ok(Some(Ok(Message::Text(t)))) => {
                            if let Some(reply) = self.dispatch(&t) { let _ = tx.send(reply); }
                        }
                        Ok(Some(Ok(Message::Close(frame)))) => {
                            break if frame.is_some_and(|f| u16::from(f.code) == 4401) { Err(true) } else { Ok(()) };
                        }
                        Ok(Some(Ok(_))) => {}
                    }
                }
            }
        };
        self.state.lock().unwrap().tx = None;
        self.set_connected(false);
        result
    }

    async fn run(self: Arc<Self>) {
        let mut delay = 1u64;
        loop {
            if !self.state.lock().unwrap().want {
                break;
            }
            let Some(i) = self.owner.upgrade() else { break };
            let Some(token) = i.token() else { break };
            let base = i.base_url();
            drop(i);
            let url = format!("{}/ws", base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1));
            let started = Instant::now();
            match self.session(&url, &token).await {
                Err(true) => break,
                _ => {
                    if started.elapsed() > Duration::from_secs(30) {
                        delay = 1;
                    }
                }
            }
            if !self.state.lock().unwrap().want {
                break;
            }
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
        self.state.lock().unwrap().task = None;
    }
}

#[uniffi::export]
impl LiveSocket {
    /// Starts (or keeps) the connection; safe to call repeatedly.
    pub fn connect(&self) {
        let mut s = self.state.lock().unwrap();
        s.want = true;
        if s.task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let Some(me) = self.me.lock().unwrap().upgrade() else { return };
        s.task = Some(RT.spawn(me.run()));
    }

    pub fn disconnect(&self) {
        let task = {
            let mut s = self.state.lock().unwrap();
            s.want = false;
            s.tx = None;
            s.task.take()
        };
        if let Some(t) = task {
            t.abort();
        }
        self.set_connected(false);
    }

    pub fn is_connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }

    pub fn add_connection_observer(&self, observer: Arc<dyn LiveConnectionObserver>) {
        self.state.lock().unwrap().observers.push(observer);
    }

    /// Watch `key` (replaces any previous listener for it).
    pub fn watch(&self, key: String, params: Option<HashMap<String, String>>, listener: Arc<dyn LiveListener>) {
        let frame = watch_frame(&key, &params);
        self.state.lock().unwrap().watches.insert(key, Watch { params, listener });
        self.send(frame);
    }

    /// Change a watch's params, keeping its listener.
    pub fn resubscribe(&self, key: String, params: Option<HashMap<String, String>>) {
        let frame = {
            let mut s = self.state.lock().unwrap();
            let Some(w) = s.watches.get_mut(&key) else { return };
            w.params = params.clone();
            watch_frame(&key, &params)
        };
        self.send(frame);
    }

    pub fn unwatch(&self, key: String) {
        if self.state.lock().unwrap().watches.remove(&key).is_some() {
            self.send(json!({ "type": "unwatch", "key": key }).to_string());
        }
    }
}
