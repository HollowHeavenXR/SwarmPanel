"""Protocol test for core.live against a stand-in /ws server.

Checks the handshake and multiplexing contract the real backend
(lua/src/routes.lua) implements: auth frame first, watch/unwatch with
params, snapshot dispatch by key, snapshot_error routing, server ping ->
client pong, and replay of every registered watch onto a fresh connection
after a drop.
"""

import asyncio
import json
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import isolate  # noqa: F401  -- must precede every swarmpanel import

import gi
from gi.repository import GLib

import websockets

from swarmpanel.core import live as live_module
from swarmpanel.core.api import api
from swarmpanel.core.config import settings

RECEIVED = []          # frames the server got, per connection
CONNECTIONS = []       # one list of frames per connection
drop_next = threading.Event()


async def handler(socket):
    frames = []
    CONNECTIONS.append(frames)
    try:
        async for raw in socket:
            message = json.loads(raw)
            frames.append(message)
            RECEIVED.append(message)
            kind = message.get("type")
            if kind == "auth":
                assert message["token"] == "test-token"
            elif kind == "watch":
                key = message["key"]
                if key == "explodes":
                    await socket.send(json.dumps(
                        {"type": "snapshot_error", "key": key, "error": "boom"}))
                else:
                    await socket.send(json.dumps({
                        "type": "snapshot",
                        "key": key,
                        "data": {"key": key, "params": message.get("params"),
                                 "conn": len(CONNECTIONS)},
                    }))
                if key == "pinger":
                    await socket.send(json.dumps({"type": "ping"}))
            if drop_next.is_set() and kind == "watch":
                drop_next.clear()
                await socket.close(code=1011)
                return
    except websockets.exceptions.ConnectionClosed:
        pass


def main():
    loop = asyncio.new_event_loop()
    ready = threading.Event()
    port = {}

    async def boot():
        server = await websockets.serve(handler, "127.0.0.1", 0)
        port["n"] = server.sockets[0].getsockname()[1]
        ready.set()
        await asyncio.Future()  # serve until the process exits

    def run_server():
        asyncio.set_event_loop(loop)
        loop.run_until_complete(boot())

    threading.Thread(target=run_server, daemon=True).start()
    ready.wait(10)

    settings.base_url = f"http://127.0.0.1:{port['n']}"
    api._token = "test-token"

    snapshots = {}
    errors = {}
    live = live_module.LiveSocket()
    live.connect()

    live.watch("dashboard", lambda d: snapshots.setdefault("dashboard", []).append(d))
    live.watch("control_state",
               lambda d: snapshots.setdefault("control_state", []).append(d),
               params={"bot_key": "aria", "guild_id": "42"})
    live.watch("explodes", lambda d: None,
               on_error=lambda m: errors.setdefault("explodes", []).append(m))
    live.watch("pinger", lambda d: snapshots.setdefault("pinger", []).append(d))

    deadline = time.time() + 15
    main_loop = GLib.MainContext.default()
    def pump(until):
        while time.time() < deadline and not until():
            while main_loop.pending():
                main_loop.iteration(False)
            time.sleep(0.02)

    pump(lambda: len(snapshots.get("dashboard", [])) >= 1
         and len(snapshots.get("control_state", [])) >= 1
         and len(errors.get("explodes", [])) >= 1
         and any(f.get("type") == "pong" for f in RECEIVED))

    assert snapshots.get("dashboard"), "no dashboard snapshot"
    assert snapshots["control_state"][0]["params"] == {"bot_key": "aria", "guild_id": "42"}, \
        f"params not forwarded: {snapshots['control_state'][0]}"
    assert errors.get("explodes") == ["boom"], f"snapshot_error not routed: {errors}"
    assert RECEIVED[0]["type"] == "auth", f"auth not first: {RECEIVED[0]}"
    assert any(f.get("type") == "pong" for f in RECEIVED), "server ping not answered with pong"
    print("ok  auth-first, watch params, snapshot dispatch, snapshot_error, ping->pong")

    # resubscribe with new params (Controls switching bots)
    live.resubscribe("control_state", params={"bot_key": "nova", "guild_id": "99"})
    deadline = time.time() + 10
    pump(lambda: len(snapshots.get("control_state", [])) >= 2)
    assert snapshots["control_state"][-1]["params"] == {"bot_key": "nova", "guild_id": "99"}, \
        "resubscribe did not re-send new params"
    print("ok  resubscribe sends new params")

    # unwatch stops dispatch and tells the server
    live.unwatch("dashboard")
    deadline = time.time() + 10
    pump(lambda: any(f.get("type") == "unwatch" and f.get("key") == "dashboard"
                     for f in RECEIVED))
    assert any(f.get("type") == "unwatch" for f in RECEIVED), "no unwatch frame"
    print("ok  unwatch frame sent")

    # force a server-side drop, then confirm every remaining watch is replayed
    conns_before = len(CONNECTIONS)
    drop_next.set()
    live.resubscribe("pinger")
    deadline = time.time() + 25
    pump(lambda: len(CONNECTIONS) > conns_before
         and len([f for f in CONNECTIONS[-1] if f.get("type") == "watch"]) >= 3)
    replayed = {f["key"] for f in CONNECTIONS[-1] if f.get("type") == "watch"}
    assert CONNECTIONS[-1][0]["type"] == "auth", "reconnect did not re-auth first"
    assert {"control_state", "explodes", "pinger"} <= replayed, \
        f"watches not replayed after reconnect: {replayed}"
    assert "dashboard" not in replayed, "unwatched key was replayed"
    print(f"ok  reconnect re-auths and replays watches {sorted(replayed)}")

    live.disconnect()
    print("\nALL LIVE PROTOCOL TESTS PASSED")


if __name__ == "__main__":
    main()
