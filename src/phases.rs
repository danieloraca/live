use crate::util::{number, object};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    values: [Option<bool>; 2],
}

impl State {
    pub fn json(&self) -> String {
        object(&[
            ("yin", number(self.values[0])),
            ("yang", number(self.values[1])),
        ])
    }
}

// Each stream restores its own last event, then follows new events without a
// polling gap. The streams are shared by all browsers and independent of metrics.
pub fn watch(state: Arc<RwLock<State>>, stopping: Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
    ["u1", "u2"]
        .into_iter()
        .enumerate()
        .map(|(index, user)| {
            let state = Arc::clone(&state);
            let stopping = Arc::clone(&stopping);
            thread::spawn(move || follow(user, index, state, stopping))
        })
        .collect()
}

fn follow(user: &'static str, index: usize, state: Arc<RwLock<State>>, stopping: Arc<AtomicBool>) {
    let pattern = format!(
        "^\\x07*([0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}} [0-9]{{2}}:[0-9]{{2}}:[0-9]{{2}} )?{user} (in|out)$"
    );
    while !stopping.load(Ordering::Relaxed) {
        let child = Command::new("journalctl")
            .args([
                "--unit=comm.service",
                "--no-pager",
                "--quiet",
                "--all",
                "--output=cat",
                "--lines=1",
                "--follow",
                "--case-sensitive=yes",
                "--grep",
                &pattern,
            ])
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            let stdout = child.stdout.take().unwrap();
            let (sender, receiver) = mpsc::sync_channel(16);
            let reader = thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut message = String::new();
                    // Bound malformed journal messages as well as queued events.
                    match reader.by_ref().take(4096).read_line(&mut message) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if !message.ends_with('\n') => break,
                        Ok(_) => {
                            if let Some(value) = parse(&message, user)
                                && sender.send(value).is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            });
            while !stopping.load(Ordering::Relaxed) {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(value) => state.write().unwrap().values[index] = Some(value),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if !matches!(child.try_wait(), Ok(None)) {
                            break;
                        }
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            drop(receiver);
            let _ = reader.join();
        }
        state.write().unwrap().values[index] = None;
        // A lost journal reader clears its phase, then restores the latest
        // retained event on reconnect. Shutdown does not wait through the retry.
        let retry = Instant::now() + Duration::from_secs(1);
        while !stopping.load(Ordering::Relaxed) && Instant::now() < retry {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn parse(message: &str, user: &str) -> Option<bool> {
    // comm prefixes its messages with a terminal bell (observed on the Pi).
    let fields: Vec<_> = message
        .trim_start_matches('\u{7}')
        .split_whitespace()
        .collect();
    let (event_user, event) = match fields.as_slice() {
        [event_user, event] => (*event_user, *event),
        [date, time, event_user, event]
            if timestamp_part(date, b"0000-00-00") && timestamp_part(time, b"00:00:00") =>
        {
            (*event_user, *event)
        }
        _ => return None,
    };
    if event_user != user {
        return None;
    }
    match event {
        "in" => Some(true),
        "out" => Some(false),
        _ => None,
    }
}

fn timestamp_part(value: &str, pattern: &[u8]) -> bool {
    value.len() == pattern.len()
        && value
            .bytes()
            .zip(pattern)
            .all(|(value, &expected)| match expected {
                b'0' => value.is_ascii_digit(),
                _ => value == expected,
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_timestamped_comm_events_to_independent_phases() {
        for (user, event, expected) in [
            ("u1", "in", true),
            ("u1", "out", false),
            ("u2", "in", true),
            ("u2", "out", false),
        ] {
            let message = format!("2026-10-31 15:06:37 {user} {event}");
            assert_eq!(parse(&message, user), Some(expected));
            let other = if user == "u1" { "u2" } else { "u1" };
            assert_eq!(parse(&message, other), None);
        }
    }

    #[test]
    fn also_accepts_messages_without_an_application_timestamp() {
        assert_eq!(parse("u1 in\n", "u1"), Some(true));
        assert_eq!(parse("u2 out", "u2"), Some(false));
    }

    #[test]
    fn accepts_the_bell_prefix_emitted_by_comm_on_the_pi() {
        assert_eq!(parse("\u{7}2026-10-31 15:04:10 u2 in", "u2"), Some(true));
        assert_eq!(parse("\u{7}2026-10-31 15:06:52 u1 out", "u1"), Some(false));
    }

    #[test]
    fn missing_malformed_and_unrelated_messages_are_unknown_not_off() {
        for message in [
            "",
            "-- No entries --",
            "comm.service: Started",
            "debug message u1 in",
            "2026-10-31 invalid u1 in",
            "2026-10-31 15:04:10 u10 in",
            "2026-10-31 15:04:10 u1 inside",
            "2026-10-31 15:04:10 u1 IN",
            "2026-10-31 15:04:10 u1 out extra",
            "u1 in\nu1 out",
        ] {
            assert_eq!(parse(message, "u1"), None, "{message}");
        }
    }
}
