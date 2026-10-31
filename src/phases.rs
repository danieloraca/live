use crate::util::{command, number, object};

// Read each user's most recent matching event independently: a busy u2 must
// never push u1's last event out of a shared, fixed-size log window.
pub fn collect() -> String {
    object(&[
        ("yin", number(latest("u1"))),
        ("yang", number(latest("u2"))),
    ])
}

fn latest(user: &str) -> Option<bool> {
    let pattern = format!(
        "^\\x07*([0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}} [0-9]{{2}}:[0-9]{{2}}:[0-9]{{2}} )?{user} (in|out)$"
    );
    let output = command(
        "journalctl",
        &[
            "--unit=comm.service",
            "--no-pager",
            "--quiet",
            "--all",
            "--output=cat",
            "--lines=1",
            "--case-sensitive=yes",
            "--grep",
            &pattern,
        ],
    )?;
    parse(&output, user)
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
