"""Run blocking work off the GTK main thread.

The REST client is synchronous, so every screen that loads or mutates data
does it here and gets the result back on the main thread. Replaces the
`Task { ... }` / `@MainActor` pairing the SwiftUI view models use, including
their cancellation checks (CancellationCheck.swift): a screen that has gone
away must not apply a late response.
"""

from __future__ import annotations

import threading
from typing import Any, Callable

from gi.repository import GLib

from .api import ApiError


class Job:
    """Handle for in-flight work; `cancel()` makes the result be discarded.

    Cancellation is cooperative and only suppresses delivery -- it cannot
    interrupt a request already in flight. That is enough for the UI's
    purpose (don't apply a stale response to a screen the user left) and
    avoids the thread-killing that Python doesn't safely allow anyway.
    """

    __slots__ = ("_cancelled",)

    def __init__(self) -> None:
        self._cancelled = False

    @property
    def cancelled(self) -> bool:
        return self._cancelled

    def cancel(self) -> None:
        self._cancelled = True


def run(
    work: Callable[[], Any],
    on_success: Callable[[Any], None] | None = None,
    on_error: Callable[[str], None] | None = None,
    on_finally: Callable[[], None] | None = None,
) -> Job:
    """Call `work()` on a worker thread; deliver to callbacks on the main thread.

    `on_error` receives the already-user-facing message -- ApiError.message
    for backend failures, `str(exc)` otherwise -- so callers can pipe it
    straight into an error banner or toast.
    """
    job = Job()

    def target() -> None:
        try:
            result = work()
        except ApiError as error:
            if not job.cancelled:
                GLib.idle_add(_deliver, job, on_error, error.message, on_finally)
            return
        except Exception as error:  # noqa: BLE001 - surfaced to the user
            if not job.cancelled:
                message = str(error) or error.__class__.__name__
                GLib.idle_add(_deliver, job, on_error, message, on_finally)
            return
        if not job.cancelled:
            GLib.idle_add(_deliver, job, on_success, result, on_finally)

    threading.Thread(target=target, name="swarmpanel-work", daemon=True).start()
    return job


def _deliver(job: Job, callback, value, on_finally) -> bool:
    if job.cancelled:
        return GLib.SOURCE_REMOVE
    try:
        if callback is not None:
            callback(value)
    except Exception:
        import traceback

        traceback.print_exc()
    finally:
        if on_finally is not None:
            try:
                on_finally()
            except Exception:
                import traceback

                traceback.print_exc()
    return GLib.SOURCE_REMOVE


def poll(interval_seconds: float, tick: Callable[[], bool]) -> int:
    """REST fallback polling for when the live socket is down.

    `tick` returns True to keep polling. Returns the GLib source id so the
    caller can `GLib.source_remove` it when the screen goes away.
    """
    return GLib.timeout_add_seconds(int(interval_seconds), tick)
