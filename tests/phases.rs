#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    address: String,
}

impl Server {
    fn start(directory: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        let mut paths = vec![directory.to_path_buf()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let child = Command::new(env!("CARGO_BIN_EXE_live"))
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("PHASE_TEST_JOURNAL", directory)
            .env("LIVE_ADDRESS", &address)
            .env("LIVE_HISTORY_DB", directory.join("history.sqlite3"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self { child, address }
    }

    fn phases(&self) -> Option<Value> {
        let mut stream = TcpStream::connect(&self.address).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .ok()?;
        stream
            .write_all(b"GET /api/phases HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).ok()?;
        if !response.starts_with("HTTP/1.1 200 ") {
            return None;
        }
        serde_json::from_str(response.split_once("\r\n\r\n")?.1).ok()
    }

    fn expect(&mut self, expected: Value) {
        self.expect_within(expected, Duration::from_secs(3));
    }

    fn expect_within(&mut self, expected: Value, timeout: Duration) {
        let started = Instant::now();
        let deadline = started + timeout;
        let mut last = None;
        while Instant::now() < deadline {
            assert!(self.child.try_wait().unwrap().is_none(), "server exited");
            last = self.phases();
            if last.as_ref() == Some(&expected) {
                eprintln!("phases {expected} received after {:?}", started.elapsed());
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("expected {expected}; last phases: {last:?}");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Exercise graceful shutdown so the same database can be reopened.
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                if !thread::panicking() {
                    assert!(status.success(), "unclean shutdown: {status}");
                }
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        assert!(thread::panicking(), "server did not shut down in time");
    }
}

fn event(directory: &Path, phase: &str, message: &str) {
    let temporary = directory.join("event.tmp");
    fs::write(&temporary, message).unwrap();
    fs::rename(temporary, directory.join(phase)).unwrap();
}

#[test]
fn comm_events_reach_the_status_api_and_restore_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path();
    let journalctl = directory.join("journalctl");
    // Substitute only the journal command. Run the real sampler and HTTP server.
    fs::write(
        &journalctl,
        r#"#!/bin/sh
test "$1" = '--unit=comm.service' || exit 1
case "$*" in
  *'--output=cat'*'--lines=1'*'--follow'*'--case-sensitive=yes'*'--grep'*) ;;
  *) exit 1 ;;
esac
for pattern in "$@"; do
  case "$pattern" in
    *'u1 (in|out)$') phase=yin; break ;;
    *'u2 (in|out)$') phase=yang; break ;;
  esac
done
test -n "$phase" || exit 1
last=unread
while test ! -f "$PHASE_TEST_JOURNAL/fail"; do
  current=$(cat "$PHASE_TEST_JOURNAL/$phase")
  if test "$current" != "$last"; then
    printf '%s\n' "$current"
    last=$current
  fi
  sleep 0.02
done
exit 1
"#,
    )
    .unwrap();
    fs::set_permissions(&journalctl, fs::Permissions::from_mode(0o755)).unwrap();

    event(directory, "yin", "\u{7}2026-10-31 15:06:37 u1 in\n");
    event(directory, "yang", "\u{7}2026-10-31 16:09:41 u2 out\n");
    let mut server = Server::start(directory);
    server.expect(json!({"yin": true, "yang": false}));

    event(directory, "yang", "\u{7}2026-10-31 16:10:00 u2 in\n");
    server.expect_within(json!({"yin": true, "yang": true}), Duration::from_secs(1));
    event(directory, "yin", "\u{7}2026-10-31 16:10:10 u1 out\n");
    server.expect_within(json!({"yin": false, "yang": true}), Duration::from_secs(1));
    event(directory, "yang", "\u{7}2026-10-31 16:10:20 u2 out\n");
    server.expect_within(json!({"yin": false, "yang": false}), Duration::from_secs(1));

    event(directory, "yin", "unrelated log message\n");
    // Wait through a fixture read, then confirm noise did not change the state.
    thread::sleep(Duration::from_millis(100));
    server.expect(json!({"yin": false, "yang": false}));
    fs::write(directory.join("fail"), "").unwrap();
    server.expect(json!({"yin": null, "yang": null}));
    event(directory, "yin", "u1 in\n");
    event(directory, "yang", "");
    fs::remove_file(directory.join("fail")).unwrap();
    server.expect(json!({"yin": true, "yang": null}));

    drop(server);
    let mut restarted = Server::start(directory);
    restarted.expect(json!({"yin": true, "yang": null}));
}
