"""Exercise the real server, its SQLite file, and graceful restart using only stdlib."""

import contextlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
subprocess.run(["cargo", "build", "--offline"], cwd=ROOT, check=True)


@contextlib.contextmanager
def server(database, log):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = dict(os.environ, LIVE_ADDRESS=f"127.0.0.1:{port}", LIVE_HISTORY_DB=str(database))
    process = subprocess.Popen([ROOT / "target/debug/live"], env=env, stdout=log, stderr=log)

    def get(path):
        with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=3) as response:
            return json.load(response)

    try:
        deadline = time.monotonic() + 20
        while True:
            assert process.poll() is None, "server exited before becoming ready"
            try:
                get("/api/status")
                break
            except (urllib.error.URLError, TimeoutError):
                assert time.monotonic() < deadline, "server did not become ready"
                time.sleep(0.1)
        # Accepted streams must still allow headers arriving in separate packets.
        with socket.create_connection(("127.0.0.1", port), timeout=3) as client:
            client.sendall(b"GET /api/history HTTP/1.1\r\nHost: localhost\r\n")
            time.sleep(0.05)
            client.sendall(b"\r\n")
            response = b""
            while b"\r\n" not in response:
                part = client.recv(1024)
                assert part, "server closed before sending a status line"
                response += part
            assert response.startswith(b"HTTP/1.1 200 OK\r\n"), response
        yield get
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
        try:
            assert process.wait(timeout=15) == 0, "server did not shut down cleanly"
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


with tempfile.TemporaryDirectory(prefix="live-history-test-") as directory:
    database = Path(directory) / "history.sqlite3"
    with tempfile.TemporaryFile(mode="w+") as log:
        try:
            with server(database, log) as get:
                # The first sample is durable; the next one must remain in a batch.
                deadline = time.monotonic() + 15
                while True:
                    status = get("/api/status")
                    if status["history"]["pending_samples"]:
                        break
                    assert time.monotonic() < deadline, "no pending sample appeared"
                    time.sleep(0.2)
                pending_timestamp = status["metrics"]["timestamp"]
                points = get("/api/history")["points"]
                assert any(p["timestamp"] == pending_timestamp for p in points)
                with sqlite3.connect(database) as connection:
                    assert connection.execute("SELECT COUNT(*) FROM samples WHERE timestamp = ?", (pending_timestamp,)).fetchone()[0] == 0
                try:
                    get("/api/history?minutes=999999999")
                    raise AssertionError("unbounded range was accepted")
                except urllib.error.HTTPError as error:
                    assert error.code == 400

            # SIGTERM must have committed that pending sample, before the 30s timer.
            with sqlite3.connect(database) as connection:
                assert connection.execute("SELECT COUNT(*) FROM samples WHERE timestamp = ?", (pending_timestamp,)).fetchone()[0] == 1
                old = int(time.time()) - 400 * 86400
                connection.execute("INSERT INTO samples VALUES (?, 42, 100, 50)", (old,))
                day_start = (int(time.time()) // 60 - 120) * 60
                connection.executemany("INSERT INTO samples VALUES (?, ?, ?, ?)", [(day_start, 20, 100, None), (day_start + 5, 40, 300, None)])

            with server(database, log) as get:
                points = get("/api/history")["points"]
                assert any(p["timestamp"] == pending_timestamp for p in points)
                day = get("/api/history?minutes=1440")
                assert day["resolution_seconds"] == 60
                bucket = next(p for p in day["points"] if p["timestamp"] == day_start + 5)
                assert bucket == {"timestamp": day_start + 5, "cpu": 30, "rx": 200, "tx": None}
                assert get("/api/status")["history"]["state"] == "ok"
                for minutes, resolution in [(15, 5), (60, 5), (10080, 600), (43200, 1800)]:
                    result = get(f"/api/history?minutes={minutes}")
                    assert result["resolution_seconds"] == resolution
                with sqlite3.connect(database) as connection:
                    assert connection.execute("SELECT cpu FROM samples WHERE timestamp = ?", (old,)).fetchone() == (42,)
                    assert connection.execute("PRAGMA integrity_check").fetchone() == ("ok",)
            print("PASS: pending data, atomic shutdown flush, restart, indefinite retention, range averages, HTTP validation, SQLite integrity")
        except BaseException:
            log.seek(0)
            print(log.read())
            raise
