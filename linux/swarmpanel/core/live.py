"""Live snapshot feed -- port of SwarmLiveSocket.swift (itself a port of
static/app.js's swarmLiveConnect()/window.swarmLive).

One shared `/ws` connection for the whole app, multiplexed by "watch" key
(dashboard, control_state, notifications, thread_messages, ...). Sends
`{"type":"auth","token":...}` as the first frame, then
`{"type":"watch"/"unwatch","key":...,"params":...}` per subscription, and
dispatches `{"type":"snapshot"/"snapshot_error","key":...}` to the one
handler registered for that key.

Last watcher wins per key -- same as app.js's `handlers[key] = handler`,
which exists specifically so a screen resubscribing with new params (Controls
switching bots) doesn't accumulate duplicate handlers.

Threading: websockets is asyncio, GTK is not. The connection lives on a
dedicated daemon thread running its own event loop; handlers are always
invoked back on the GTK main thread via GLib.idle_add, so screen code can
touch widgets directly without any locking of its own.
"""

from __future__ import annotations

import asyncio
import json
import threading
import time
from typing import Any, Callable, Mapping

from gi.repository import GLib

from .api import api
from .config import settings

SnapshotHandler = Callable[[Any], None]
ErrorHandler = Callable[[str], None]

# routes.lua's /ws loop sends a {"type":"ping"} after 45s of no inbound
# frame, so a healthy connection always delivers *something* at least that
# often. If nothing arrives for well over that window the socket is dead in
# a way the library hasn't reported -- a dropped cloudflared tunnel, a
# suspend/resume, a NAT timeout -- which would otherwise leave `connected`
# stuck true, so neither the reconnect path nor the screens' REST fallback
# poll would ever run and the dashboard would quietly show stale data.
STALE_FRAME_TIMEOUT = 120.0
WATCHDOG_INTERVAL = 15.0
MAX_RECONNECT_DELAY = 30.0


class _Watch:
    __slots__ = ("params", "on_snapshot", "on_error")

    def __init__(self, params, on_snapshot, on_error) -> None:
        self.params = params
        self.on_snapshot = on_snapshot
        self.on_error = on_error


class LiveSocket:
    def __init__(self) -> None:
        self._watches: dict[str, _Watch] = {}
        self._lock = threading.RLock()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread: threading.Thread | None = None
        self._socket = None
        self._should_stay_connected = False
        self._connected = False
        self._reconnect_delay = 1.0
        self._last_frame_at = time.monotonic()
        self._connection_listeners: list[Callable[[bool], None]] = []

    # -- public API (call from the GTK main thread) ------------------------

    @property
    def connected(self) -> bool:
        return self._connected

    def add_connection_listener(self, callback: Callable[[bool], None]) -> None:
        """Notified (on the main thread) whenever connectivity flips, so the
        header can show an offline pill and screens can start/stop their
        REST fallback polling."""
        self._connection_listeners.append(callback)

    def connect(self) -> None:
        self._should_stay_connected = True
        if self._thread is None or not self._thread.is_alive():
            self._thread = threading.Thread(
                target=self._run_loop, name="swarmpanel-live", daemon=True
            )
            self._thread.start()

    def disconnect(self) -> None:
        self._should_stay_connected = False
        loop = self._loop
        if loop is not None:
            loop.call_soon_threadsafe(self._close_socket_soon)
        self._set_connected(False)

    def watch(
        self,
        key: str,
        on_snapshot: SnapshotHandler,
        params: Mapping[str, str] | None = None,
        on_error: ErrorHandler | None = None,
    ) -> None:
        """Register (replacing any existing) handler for `key` and subscribe.

        Safe to call before connect()/before the socket is up: the watch is
        queued and sent on the next successful connect, the same path a
        reconnect replay takes.
        """
        with self._lock:
            self._watches[key] = _Watch(dict(params or {}) or None, on_snapshot, on_error)
        self._send_watch(key)

    def resubscribe(self, key: str, params: Mapping[str, str] | None = None) -> None:
        """Re-send a watch for an already-registered key with new params --
        e.g. Controls switching bot_key/guild_id. No-op for an unknown key,
        mirroring app.js's resubscribe()."""
        with self._lock:
            existing = self._watches.get(key)
            if existing is None:
                return
            existing.params = dict(params or {}) or None
        self._send_watch(key)

    def unwatch(self, key: str) -> None:
        with self._lock:
            if self._watches.pop(key, None) is None:
                return
        self._send(json.dumps({"type": "unwatch", "key": key}))

    # -- event loop thread -------------------------------------------------

    def _run_loop(self) -> None:
        loop = asyncio.new_event_loop()
        self._loop = loop
        asyncio.set_event_loop(loop)
        try:
            loop.run_until_complete(self._supervise())
        finally:
            try:
                loop.run_until_complete(loop.shutdown_asyncgens())
            finally:
                loop.close()
                self._loop = None

    async def _supervise(self) -> None:
        """Connect/reconnect forever with exponential backoff, and run the
        staleness watchdog alongside."""
        watchdog = asyncio.ensure_future(self._watchdog())
        try:
            while self._should_stay_connected:
                token = api.token
                if not token:
                    # Not signed in yet; wait rather than hammering /ws with
                    # auth frames that can only fail.
                    await asyncio.sleep(1.0)
                    continue
                try:
                    await self._session(token)
                except asyncio.CancelledError:
                    raise
                except Exception:
                    pass
                self._set_connected(False)
                if not self._should_stay_connected:
                    break
                delay = self._reconnect_delay
                self._reconnect_delay = min(self._reconnect_delay * 2, MAX_RECONNECT_DELAY)
                await asyncio.sleep(delay)
        finally:
            watchdog.cancel()

    def _ws_url(self) -> str:
        base = settings.base_url
        if base.startswith("https://"):
            return "wss://" + base[len("https://") :] + "/ws"
        if base.startswith("http://"):
            return "ws://" + base[len("http://") :] + "/ws"
        return "wss://" + base + "/ws"

    async def _session(self, token: str) -> None:
        import websockets

        # Disable the library's own keepalive: this protocol has its own
        # application-level {"type":"ping"} from the server that we answer
        # with a pong frame, and the watchdog is what decides liveness.
        async with websockets.connect(
            self._ws_url(),
            ping_interval=None,
            open_timeout=20,
            max_size=16 * 1024 * 1024,
            user_agent_header="SwarmPanel-Linux/1.0",
        ) as socket:
            self._socket = socket
            await socket.send(json.dumps({"type": "auth", "token": token}))
            # A brand-new socket only auto-subscribes "dashboard"
            # server-side, so replay everything else this app had watched
            # before the drop.
            await self._resend_all_watches(socket)
            self._reconnect_delay = 1.0
            self._last_frame_at = time.monotonic()
            self._set_connected(True)
            try:
                async for raw in socket:
                    # Stamped for every frame type, including ones _handle
                    # ignores, so the watchdog measures connection liveness
                    # rather than "did a watched key change recently".
                    self._last_frame_at = time.monotonic()
                    self._handle(raw)
            finally:
                self._socket = None
                self._set_connected(False)

    async def _resend_all_watches(self, socket) -> None:
        with self._lock:
            pending = [(key, watch.params) for key, watch in self._watches.items()]
        for key, params in pending:
            await socket.send(self._watch_frame(key, params))

    async def _watchdog(self) -> None:
        """Force a reconnect when frames stop arriving, covering the
        half-open-socket case the library never reports."""
        while True:
            await asyncio.sleep(WATCHDOG_INTERVAL)
            if not (self._should_stay_connected and self._connected):
                continue
            if time.monotonic() - self._last_frame_at > STALE_FRAME_TIMEOUT:
                # A stall is not a backoff-worthy failure -- the network is
                # usually fine by now -- so retry promptly rather than at
                # whatever delay an earlier failure had escalated to.
                self._reconnect_delay = 1.0
                self._close_socket_soon()

    def _close_socket_soon(self) -> None:
        socket = self._socket
        if socket is None:
            return
        # 1006 ("abnormal closure") is reserved: it describes a connection
        # that dropped *without* a close frame, so it can never be sent in
        # one. Using it raised ProtocolError inside the close, which meant
        # the watchdog's forced reconnect -- the whole point of which is to
        # recover a half-open socket -- threw instead of reconnecting.
        # 1001 ("going away") is the correct code for a client-side teardown.
        asyncio.ensure_future(socket.close(code=1001))

    # -- frames ------------------------------------------------------------

    @staticmethod
    def _watch_frame(key: str, params: Mapping[str, str] | None) -> str:
        frame: dict[str, Any] = {"type": "watch", "key": key}
        if params:
            frame["params"] = dict(params)
        return json.dumps(frame)

    def _send_watch(self, key: str) -> None:
        with self._lock:
            watch = self._watches.get(key)
            if watch is None:
                return
            params = watch.params
        self._send(self._watch_frame(key, params))

    def _send(self, text: str) -> None:
        loop, socket = self._loop, self._socket
        if loop is None or socket is None:
            # Queued: it will be replayed by _resend_all_watches on connect.
            return
        loop.call_soon_threadsafe(lambda: asyncio.ensure_future(self._send_now(text)))

    async def _send_now(self, text: str) -> None:
        socket = self._socket
        if socket is None:
            return
        try:
            await socket.send(text)
        except Exception:
            pass

    def _handle(self, raw: Any) -> None:
        if isinstance(raw, (bytes, bytearray)):
            try:
                raw = raw.decode("utf-8")
            except UnicodeDecodeError:
                return
        if not isinstance(raw, str):
            return
        try:
            message = json.loads(raw)
        except ValueError:
            return
        if not isinstance(message, dict):
            return

        kind = message.get("type")
        if kind == "snapshot":
            key = message.get("key")
            watch = self._watch_for(key)
            if watch and "data" in message:
                payload = message["data"]
                GLib.idle_add(_invoke, watch.on_snapshot, payload)
        elif kind == "snapshot_error":
            watch = self._watch_for(message.get("key"))
            if watch and watch.on_error:
                text = message.get("error") or "Unknown error"
                GLib.idle_add(_invoke, watch.on_error, str(text))
        elif kind == "ping":
            asyncio.ensure_future(self._send_now(json.dumps({"type": "pong"})))

    def _watch_for(self, key: Any) -> _Watch | None:
        if not isinstance(key, str):
            return None
        with self._lock:
            return self._watches.get(key)

    def _set_connected(self, value: bool) -> None:
        if self._connected == value:
            return
        self._connected = value
        for listener in list(self._connection_listeners):
            GLib.idle_add(_invoke, listener, value)


def _invoke(callback, argument) -> bool:
    """Run a handler on the GTK main thread exactly once.

    A raising handler must not take the idle source (or the process) down,
    and must not be retried -- returning GLib.SOURCE_REMOVE unconditionally
    is what guarantees both.
    """
    try:
        callback(argument)
    except Exception:
        import traceback

        traceback.print_exc()
    return GLib.SOURCE_REMOVE


live = LiveSocket()
